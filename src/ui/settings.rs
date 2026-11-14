//! Settings: appearance, history/privacy, data locations, and destructive actions.

use std::path::Path;

use eframe::egui::{self, Align, Layout, RichText};

use crate::config::{Config, Theme};
use crate::storage::Storage;

pub struct SettingsOutcome {
    pub history_cleared: bool,
}

pub struct SettingsScreen {
    conversation_count: Option<u64>,
    confirm_clear: bool,
    status: Option<String>,
    count_loaded: bool,
}

impl SettingsScreen {
    pub fn new() -> Self {
        Self {
            conversation_count: None,
            confirm_clear: false,
            status: None,
            count_loaded: false,
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        config: &mut Config,
        storage: &Storage,
        config_path: &Path,
        database_path: &Path,
    ) -> SettingsOutcome {
        let mut outcome = SettingsOutcome {
            history_cleared: false,
        };
        if !self.count_loaded {
            self.refresh_count(storage);
            self.count_loaded = true;
        }
        config.history_retention_days = config.history_retention_days.clamp(1, 3_650);

        ui.heading("Settings");
        ui.small("Everything stays on this Mac. No telemetry leaves the application.");

        ui.add_space(12.0);
        ui.separator();
        ui.heading("Appearance");
        ui.horizontal(|ui| {
            for theme in Theme::ALL {
                if ui
                    .selectable_label(config.theme == theme, theme.label())
                    .clicked()
                    && config.theme != theme
                {
                    config.theme = theme;
                    apply_theme(ui.ctx(), theme);
                    self.status = Some(match config.save(config_path) {
                        Ok(()) => format!("Theme set to {} and saved.", theme.label()),
                        Err(error) => format!("Theme applied but could not be saved: {error}"),
                    });
                }
            }
        });
        ui.small("System follows the macOS appearance setting.");

        ui.add_space(12.0);
        ui.separator();
        ui.heading("History & privacy");
        let history_changed = ui
            .checkbox(
                &mut config.history_enabled,
                "Save conversations to this Mac",
            )
            .changed();
        ui.small(
            "Off: conversations live in memory only. No titles, messages, tool outputs, or usage records are written, and nothing returns after a restart.",
        );
        ui.add_space(6.0);

        let mut settings_dirty = history_changed;
        ui.add_enabled_ui(config.history_enabled, |ui| {
            ui.horizontal(|ui| {
                ui.label("Retention");
                let days = ui.add(
                    egui::DragValue::new(&mut config.history_retention_days)
                        .range(1..=3_650)
                        .speed(1.0)
                        .suffix(" days"),
                );
                if days.changed() {
                    config.history_retention_days = config.history_retention_days.clamp(1, 3_650);
                    settings_dirty = true;
                }
            });
            settings_dirty |= ui
                .checkbox(&mut config.save_tool_outputs, "Save tool calls and outputs")
                .changed();
            if !config.history_enabled {
                ui.small("History being off overrides this setting.");
            }
        });
        if !config.history_enabled {
            ui.small("Tool output saving is disabled while history is off.");
        }

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Saved conversations");
            ui.strong(
                self.conversation_count
                    .map(|count| count.to_string())
                    .unwrap_or_else(|| "—".to_owned()),
            );
            if ui.small_button("Refresh").clicked() {
                self.refresh_count(storage);
            }
        });
        ui.small(
            "Usage totals are stored with saved messages, so no usage is recorded while history is off.",
        );

        ui.add_space(8.0);
        if self.confirm_clear {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "This deletes every saved conversation, message, and usage record. Providers, keys, and settings are not affected.",
            );
            ui.horizontal(|ui| {
                if ui.button("Delete all history").clicked() {
                    match storage.clear_all_history() {
                        Ok(()) => {
                            self.status = Some("All saved history was deleted.".to_owned());
                            outcome.history_cleared = true;
                            self.refresh_count(storage);
                        }
                        Err(error) => {
                            self.status = Some(format!("Could not clear history: {error}"));
                        }
                    }
                    self.confirm_clear = false;
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_clear = false;
                }
            });
        } else if ui.button("Clear all history").clicked() {
            self.confirm_clear = true;
        }

        ui.add_space(10.0);
        if history_changed {
            storage.set_history_writes(config.history_enabled);
            storage.set_tool_output_writes(config.save_tool_outputs);
        }
        if settings_dirty {
            // Retention is clamped here and on load; expired rows are pruned at startup
            // rather than mid-edit, so dragging the slider cannot delete history by accident.
            self.status = Some(match config.save(config_path) {
                Ok(()) => "Settings saved.".to_owned(),
                Err(error) => format!("Could not save settings: {error}"),
            });
        }
        if ui.button("Save settings").clicked() {
            self.status = Some(match config.save(config_path) {
                Ok(()) => "Settings saved.".to_owned(),
                Err(error) => format!("Could not save settings: {error}"),
            });
        }

        ui.add_space(12.0);
        ui.separator();
        ui.heading("Data locations");
        path_row(ui, "Configuration", config_path);
        path_row(ui, "Database", database_path);
        ui.add_space(6.0);
        ui.small(
            "Provider API keys are stored separately in the macOS Keychain, scoped to each profile and endpoint origin. They are never written to config.toml, the database, or logs.",
        );

        if let Some(status) = &self.status {
            ui.add_space(8.0);
            ui.label(status);
        }
        outcome
    }

    fn refresh_count(&mut self, storage: &Storage) {
        self.conversation_count = storage.conversation_count().ok();
    }
}

fn path_row(ui: &mut egui::Ui, label: &str, path: &Path) {
    ui.horizontal(|ui| {
        ui.label(label);
        let text = path.display().to_string();
        ui.add(egui::Label::new(RichText::new(&text).monospace().small()).wrap());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.small_button("Copy").clicked() {
                ui.ctx().copy_text(text.clone());
            }
        });
    });
}

fn apply_theme(ctx: &egui::Context, theme: Theme) {
    ctx.set_theme(match theme {
        Theme::System => egui::ThemePreference::System,
        Theme::Dark => egui::ThemePreference::Dark,
        Theme::Light => egui::ThemePreference::Light,
    });
}

pub fn apply_config_theme(ctx: &egui::Context, config: &Config) {
    apply_theme(ctx, config.theme);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn settings_renders_without_touching_history() {
        let storage = crate::storage::Storage::open_in_memory().unwrap();
        let mut config = crate::config::Config::default();
        let mut screen = SettingsScreen::new();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1_080.0, 720.0),
            )),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            screen.show(
                ui,
                &mut config,
                &storage,
                std::path::Path::new("/tmp/config.toml"),
                std::path::Path::new("/tmp/nexus.db"),
            );
        });
        output.textures_delta.clear();
        assert_eq!(storage.conversation_count().unwrap(), 0);
    }

    #[test]
    fn retention_is_clamped_when_loading() {
        let config: Config = toml::from_str("history_retention_days = 0").unwrap();
        assert_eq!(config.history_retention_days, 1);
        let config: Config = toml::from_str("history_retention_days = 99999").unwrap();
        assert_eq!(config.history_retention_days, 3_650);
        let config: Config = toml::from_str("history_retention_days = -5").unwrap();
        assert_eq!(config.history_retention_days, 1);
    }
}
