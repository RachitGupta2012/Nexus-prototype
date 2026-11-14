//! Recovery screen for unreadable or invalid configuration files.
//!
//! The screen never reads, prints, or stores configuration contents beyond what
//! the sanitized error summary exposes. It cannot modify the database, provider
//! profiles, or Keychain entries; its only write is an explicit, confirmed
//! configuration reset that backs up the original file first.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use eframe::egui;

use crate::config::Config;
use crate::error::AppError;
use crate::storage::unix_time_now;

pub enum RecoveryOutcome {
    None,
    Recovered(Config),
    Quit,
}

pub struct RecoveryScreen {
    config_path: PathBuf,
    error: String,
    confirm_reset: bool,
    status: Option<String>,
    ready: Option<Config>,
    quit: bool,
    #[cfg(test)]
    spawn_external_commands: bool,
}

impl RecoveryScreen {
    pub fn from_error(config_path: PathBuf, error: &AppError) -> Self {
        Self {
            config_path,
            error: summarize(error),
            confirm_reset: false,
            status: None,
            ready: None,
            quit: false,
            #[cfg(test)]
            spawn_external_commands: true,
        }
    }

    #[cfg(test)]
    pub(crate) fn error_text(&self) -> &str {
        &self.error
    }

    /// A deterministic screen for snapshot tests: fixed path, fixed message,
    /// no filesystem access.
    #[cfg(test)]
    pub(crate) fn preview(config_path: &str, error: &str) -> Self {
        Self {
            config_path: PathBuf::from(config_path),
            error: error.to_owned(),
            confirm_reset: false,
            status: None,
            ready: None,
            quit: false,
            #[cfg(test)]
            spawn_external_commands: true,
        }
    }

    /// Report a failure that happened after the configuration became readable,
    /// such as the database refusing to open. The original error text is kept
    /// only if it contains no configuration contents.
    pub fn runtime_error(&mut self, message: String) {
        self.error = message;
        self.ready = None;
        self.confirm_reset = false;
    }

    pub fn show(&mut self, ui: &mut egui::Ui) -> RecoveryOutcome {
        if let Some(config) = self.ready.take() {
            return RecoveryOutcome::Recovered(config);
        }
        if self.quit {
            return RecoveryOutcome::Quit;
        }

        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(24.0);
            ui.heading("NEXUS could not start");
            ui.add_space(6.0);
            ui.label(
                "The configuration file could not be loaded. NEXUS has not changed it, \
                 and no conversations, provider settings, or credentials were touched.",
            );
            ui.add_space(12.0);

            ui.label("Configuration file");
            ui.horizontal(|ui| {
                ui.monospace(self.config_path.display().to_string());
                if ui.small_button("Copy path").clicked() {
                    ui.ctx().copy_text(self.config_path.display().to_string());
                }
            });
            ui.add_space(8.0);

            ui.label("Problem");
            ui.colored_label(ui.visuals().warn_fg_color, &self.error);
            ui.small("File contents are never shown here, because they can contain credentials.");
            ui.add_space(16.0);

            ui.horizontal(|ui| {
                if ui
                    .button("Retry")
                    .on_hover_text("Try loading the configuration again without changing the file")
                    .clicked()
                {
                    self.retry();
                }
                if ui.button("Open configuration folder").clicked() {
                    self.open_folder();
                }
                if ui
                    .button("Reset configuration…")
                    .on_hover_text("Back up the current file, then write default settings")
                    .clicked()
                {
                    self.confirm_reset = true;
                    self.status = None;
                }
                if ui.button("Quit").clicked() {
                    self.quit = true;
                }
            });

            if self.confirm_reset {
                ui.add_space(12.0);
                ui.separator();
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Reset replaces the configuration file with defaults. \
                     Provider endpoints, model catalogs, theme, history, and retention settings \
                     may be reset. The original file is copied to a backup next to it before \
                     anything is written.",
                );
                ui.small(
                    "Keychain credentials, the database, and saved conversations are not deleted.",
                );
                ui.horizontal(|ui| {
                    if ui.button("Back up and reset").clicked() {
                        self.reset();
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_reset = false;
                        self.status = None;
                    }
                });
            }

            if let Some(status) = &self.status {
                ui.add_space(10.0);
                ui.label(status);
            }
        });

        if let Some(config) = self.ready.take() {
            return RecoveryOutcome::Recovered(config);
        }
        if self.quit {
            return RecoveryOutcome::Quit;
        }
        RecoveryOutcome::None
    }

    fn retry(&mut self) {
        match Config::load_or_default(&self.config_path) {
            Ok(config) => {
                self.error.clear();
                self.status = Some("Configuration loaded.".to_owned());
                self.ready = Some(config);
            }
            Err(error) => {
                self.error = summarize(&error);
                self.status = Some(
                    "The configuration still cannot be loaded. It was not modified.".to_owned(),
                );
            }
        }
    }

    /// The folder the "open configuration folder" action targets.
    fn open_folder_target(&self) -> Result<&Path, String> {
        self.config_path
            .parent()
            .ok_or_else(|| "The configuration path has no parent folder.".to_owned())
    }

    fn open_folder(&mut self) {
        let parent = match self.open_folder_target() {
            Ok(parent) => parent.to_owned(),
            Err(message) => {
                self.status = Some(message);
                return;
            }
        };
        #[cfg(test)]
        if !self.spawn_external_commands {
            self.status = Some(format!("Would open {}", parent.display()));
            return;
        }
        #[cfg(target_os = "macos")]
        {
            match Command::new("open").arg(&parent).spawn() {
                Ok(_) => self.status = Some(format!("Opened {}", parent.display())),
                Err(error) => {
                    self.status = Some(format!(
                        "Could not open the folder: {error}. It is at {}.",
                        parent.display()
                    ));
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.status = Some(format!(
                "Opening folders is not supported here. The folder is {}.",
                parent.display()
            ));
        }
    }

    /// Two-step reset: copy the original to a sibling backup, then let
    /// `Config::save` replace the file with defaults via temp-file + rename.
    /// Any failure before the rename leaves the original in place.
    fn reset(&mut self) {
        self.apply_reset(|path| Config::default().save(path));
    }

    /// Reset with an injectable writer so tests can prove that a failed write
    /// never replaces the original file.
    fn apply_reset<F>(&mut self, save: F)
    where
        F: FnOnce(&Path) -> Result<(), AppError>,
    {
        if !self.confirm_reset {
            return;
        }
        let backup = next_backup_path(&self.config_path);
        if let Err(error) = fs::copy(&self.config_path, &backup) {
            self.status = Some(format!(
                "Could not back up the configuration ({error}). Nothing was changed."
            ));
            return;
        }
        let original_permissions = fs::metadata(&backup).map(|meta| meta.permissions()).ok();

        match save(&self.config_path) {
            Ok(()) => {
                if let Some(permissions) = original_permissions {
                    let _ = fs::set_permissions(&backup, permissions);
                }
                self.confirm_reset = false;
                self.error.clear();
                self.status = Some(format!(
                    "Configuration reset. The original file was kept at {}.",
                    backup.display()
                ));
                match Config::load_or_default(&self.config_path) {
                    Ok(config) => self.ready = Some(config),
                    Err(error) => self.error = summarize(&error),
                }
            }
            Err(error) => {
                self.status = Some(format!(
                    "Could not write a new configuration ({error}). \
                     The backup is at {}. Check that the folder is writable, then retry.",
                    backup.display()
                ));
            }
        }
    }
}

/// A recovery status that must stay free of configuration contents.
pub fn summarize(error: &AppError) -> String {
    match error {
        AppError::ConfigParse { path, source } => match span_position(source, path) {
            Some((line, column)) => {
                format!("The file is not valid TOML (parse error at line {line}, column {column}).")
            }
            None => "The file is not valid TOML.".to_owned(),
        },
        AppError::ConfigRead { source, .. } => match source.kind() {
            std::io::ErrorKind::NotFound => "The file disappeared while starting.".to_owned(),
            std::io::ErrorKind::PermissionDenied => {
                "Permission denied while reading the file.".to_owned()
            }
            std::io::ErrorKind::InvalidData => "The file is not valid UTF-8 text.".to_owned(),
            _ => "The file could not be read.".to_owned(),
        },
        AppError::ConfigWrite { path, source } => {
            format!(
                "Could not write the configuration at {} ({source}).",
                path.display()
            )
        }
        AppError::Storage(source) => format!("The local database could not be opened ({source})."),
        other => other.to_string(),
    }
}

/// Convert a TOML parse-error byte span into a line and column without ever
/// returning the offending text.
fn span_position(error: &toml::de::Error, path: &Path) -> Option<(usize, usize)> {
    let span = error.span()?;
    let bytes = fs::read(path).ok()?;
    let offset = span.start.min(bytes.len());
    let prefix = &bytes[..offset];
    let line = prefix.iter().filter(|byte| **byte == b'\n').count() + 1;
    let column = prefix
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(prefix.len(), |newline| prefix.len() - newline - 1)
        + 1;
    Some((line, column))
}

fn next_backup_path(path: &Path) -> PathBuf {
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let base = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.toml");
    let stamp = unix_time_now();
    for attempt in 0..100 {
        let name = if attempt == 0 {
            format!("{base}.corrupt-{stamp}")
        } else {
            format!("{base}.corrupt-{stamp}-{attempt}")
        };
        let candidate = directory.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    directory.join(format!("{base}.corrupt-{stamp}-overflow"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "nexus-recovery-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn config(&self) -> PathBuf {
            self.0.join("config.toml")
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    use std::os::unix::fs::PermissionsExt;

    fn corrupt_screen(dir: &TestDir) -> RecoveryScreen {
        let contents = "history_enabled = true\napi_key = \"sk-SECRET-VALUE\"\nthis is not toml\n";
        fs::write(dir.config(), contents).unwrap();
        let error = Config::load_or_default(&dir.config()).unwrap_err();
        RecoveryScreen::from_error(dir.config(), &error)
    }

    fn backups(dir: &TestDir) -> Vec<PathBuf> {
        fs::read_dir(&dir.0)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains(".corrupt-"))
            })
            .collect()
    }

    #[test]
    fn summary_never_contains_file_contents() {
        let dir = TestDir::new();
        let screen = corrupt_screen(&dir);
        assert!(!screen.error.contains("sk-SECRET-VALUE"));
        assert!(!screen.error.contains("this is not toml"));
        assert!(screen.error.contains("not valid TOML"));
    }

    #[test]
    fn retry_does_not_modify_a_corrupt_file() {
        let dir = TestDir::new();
        let mut screen = corrupt_screen(&dir);
        let before = fs::read(dir.config()).unwrap();
        screen.retry();
        assert!(screen.ready.is_none());
        assert_eq!(fs::read(dir.config()).unwrap(), before);
        assert!(backups(&dir).is_empty());
    }

    #[test]
    fn retry_recovers_after_the_file_is_fixed() {
        let dir = TestDir::new();
        let mut screen = corrupt_screen(&dir);
        screen.retry();
        assert!(screen.ready.is_none());
        fs::write(dir.config(), "theme = \"dark\"\n").unwrap();
        screen.retry();
        assert!(screen.ready.is_some());
        assert!(!dir.0.join("config.toml.tmp").exists());
    }

    #[test]
    fn quit_never_touches_the_file() {
        let dir = TestDir::new();
        let mut screen = corrupt_screen(&dir);
        let before = fs::read(dir.config()).unwrap();
        screen.quit = true;
        assert_eq!(fs::read(dir.config()).unwrap(), before);
        assert!(backups(&dir).is_empty());
    }

    #[test]
    fn reset_requires_confirmation() {
        let dir = TestDir::new();
        let mut screen = corrupt_screen(&dir);
        let before = fs::read(dir.config()).unwrap();
        screen.reset();
        assert_eq!(fs::read(dir.config()).unwrap(), before);
        assert!(backups(&dir).is_empty());
    }

    #[test]
    fn confirmed_reset_keeps_a_backup_and_writes_valid_defaults() {
        let dir = TestDir::new();
        let mut screen = corrupt_screen(&dir);
        let before = fs::read(dir.config()).unwrap();
        screen.confirm_reset = true;
        screen.reset();
        let saved = backups(&dir);
        assert_eq!(saved.len(), 1);
        assert_eq!(fs::read(&saved[0]).unwrap(), before);
        assert!(Config::load_or_default(&dir.config()).is_ok());
        assert!(screen.ready.is_some());
        assert!(!dir.0.join("config.toml.tmp").exists());
    }

    #[test]
    fn failed_backup_prevents_the_destructive_reset() {
        let dir = TestDir::new();
        let mut screen = corrupt_screen(&dir);
        let before = fs::read(dir.config()).unwrap();
        fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o500)).unwrap();
        screen.confirm_reset = true;
        screen.reset();
        fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(fs::read(dir.config()).unwrap(), before);
        assert!(screen.ready.is_none());
        let status = screen.status.unwrap_or_default();
        assert!(status.contains("Nothing was changed"));
    }

    struct UiState {
        screen: RecoveryScreen,
        outcome: RecoveryOutcome,
    }

    fn harness(dir: &TestDir) -> egui_kittest::Harness<'static, UiState> {
        let screen = corrupt_screen(dir);
        egui_kittest::Harness::builder()
            .with_size(egui::vec2(1_080.0, 720.0))
            .with_theme(egui::Theme::Dark)
            .build_ui_state(
                |ui, state: &mut UiState| {
                    let outcome = state.screen.show(ui);
                    if !matches!(outcome, RecoveryOutcome::None) {
                        state.outcome = outcome;
                    }
                },
                UiState {
                    screen,
                    outcome: RecoveryOutcome::None,
                },
            )
    }

    use egui_kittest::kittest::Queryable;

    fn click(harness: &mut egui_kittest::Harness<'static, UiState>, label: &str) {
        harness.get_by_label(label).click_accesskit();
        harness.run();
    }

    #[test]
    fn recovery_window_opens_for_malformed_toml_without_showing_contents() {
        let dir = TestDir::new();
        let mut harness = harness(&dir);
        harness.run();
        assert!(harness.query_by_label("NEXUS could not start").is_some());
        assert!(
            harness
                .query_all_by_label_contains("not valid TOML")
                .next()
                .is_some(),
            "the sanitized parse error should be visible"
        );
        assert!(
            harness
                .query_all_by_label("sk-SECRET-VALUE")
                .next()
                .is_none(),
            "recovery must never render configuration contents"
        );
        assert!(matches!(harness.state().outcome, RecoveryOutcome::None));
    }

    #[test]
    fn retry_preserves_the_original_and_recovers_after_a_fix() {
        let dir = TestDir::new();
        let mut harness = harness(&dir);
        let before = fs::read(dir.config()).unwrap();
        click(&mut harness, "Retry");
        assert_eq!(fs::read(dir.config()).unwrap(), before);
        assert!(backups(&dir).is_empty());
        assert!(harness.state().screen.ready.is_none());

        fs::write(dir.config(), "theme = \"dark\"\n").unwrap();
        click(&mut harness, "Retry");
        assert!(matches!(
            harness.state().outcome,
            RecoveryOutcome::Recovered(_)
        ));
    }

    #[test]
    fn open_folder_uses_the_configuration_directory_and_copy_path_copies_it() {
        let dir = TestDir::new();
        let mut harness = harness(&dir);
        harness.state_mut().screen.spawn_external_commands = false;
        click(&mut harness, "Open configuration folder");
        assert_eq!(
            harness.state().screen.open_folder_target().unwrap(),
            dir.0.as_path()
        );
        assert!(
            harness
                .state()
                .screen
                .status
                .as_deref()
                .unwrap_or_default()
                .contains(&dir.0.display().to_string())
        );

        harness.get_by_label("Copy path").click_accesskit();
        harness.step();
        let expected = dir.config().display().to_string();
        let copied = harness.output().platform_output.commands.iter().any(
            |command| matches!(command, egui::OutputCommand::CopyText(text) if text == &expected),
        );
        assert!(copied, "Copy path must copy the real configuration path");
    }

    #[test]
    fn reset_needs_confirmation_and_cancel_keeps_the_file() {
        let dir = TestDir::new();
        let mut harness = harness(&dir);
        let before = fs::read(dir.config()).unwrap();
        click(&mut harness, "Reset configuration…");
        assert!(harness.query_by_label("Back up and reset").is_some());
        click(&mut harness, "Cancel");
        assert!(harness.query_by_label("Back up and reset").is_none());
        assert_eq!(fs::read(dir.config()).unwrap(), before);
        assert!(backups(&dir).is_empty());
        assert!(matches!(harness.state().outcome, RecoveryOutcome::None));
    }

    #[test]
    fn confirmed_reset_backs_up_first_then_recovers() {
        let dir = TestDir::new();
        let mut harness = harness(&dir);
        let before = fs::read(dir.config()).unwrap();
        click(&mut harness, "Reset configuration…");
        click(&mut harness, "Back up and reset");
        let saved = backups(&dir);
        assert_eq!(saved.len(), 1);
        assert_eq!(fs::read(&saved[0]).unwrap(), before);
        assert!(Config::load_or_default(&dir.config()).is_ok());
        assert!(matches!(
            harness.state().outcome,
            RecoveryOutcome::Recovered(_)
        ));
    }

    #[test]
    fn failed_write_preserves_the_original_and_leaves_a_backup() {
        let dir = TestDir::new();
        let mut screen = corrupt_screen(&dir);
        let before = fs::read(dir.config()).unwrap();
        screen.confirm_reset = true;
        screen.apply_reset(|path| {
            Err(AppError::ConfigWrite {
                path: path.to_owned(),
                source: std::io::Error::other("disk full"),
            })
        });
        assert_eq!(fs::read(dir.config()).unwrap(), before);
        assert!(screen.ready.is_none());
        assert_eq!(backups(&dir).len(), 1);
        let status = screen.status.unwrap_or_default();
        assert!(status.contains("Could not write"), "{status}");
        assert!(status.contains("backup is at"), "{status}");
    }

    #[test]
    fn quit_click_exits_without_modifying_the_original() {
        let dir = TestDir::new();
        let mut harness = harness(&dir);
        let before = fs::read(dir.config()).unwrap();
        click(&mut harness, "Quit");
        assert!(matches!(harness.state().outcome, RecoveryOutcome::Quit));
        assert_eq!(fs::read(dir.config()).unwrap(), before);
        assert!(backups(&dir).is_empty());
    }

    #[test]
    fn read_errors_are_described_without_contents() {
        let dir = TestDir::new();
        let unreadable = dir.0.join("unreadable.toml");
        fs::write(&unreadable, "theme = \"dark\"\n").unwrap();
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
        let error = Config::load_or_default(&unreadable).unwrap_err();
        let text = summarize(&error);
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(text.contains("Permission denied"), "{text}");
    }
}
