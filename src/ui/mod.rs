use std::path::{Path, PathBuf};

use eframe::egui;

use crate::config::Config;
use crate::error::AppError;
use crate::storage::{DEFAULT_MAX_CONVERSATIONS, Storage};

mod chat;
mod mcp;
mod providers;
mod recovery;
mod settings;
#[cfg(test)]
mod tests;
mod usage;

/// What the process should start as. A configuration that cannot be read or
/// parsed becomes a recoverable screen instead of a process exit.
pub enum Start {
    Ready(Config, Storage),
    Recover(AppError),
}

pub fn load_start(config_path: &Path, database_path: &Path) -> Result<Start, AppError> {
    match Config::load_or_default(config_path) {
        Ok(config) => Ok(Start::Ready(config, Storage::open(database_path)?)),
        Err(error @ (AppError::ConfigParse { .. } | AppError::ConfigRead { .. })) => {
            Ok(Start::Recover(error))
        }
        Err(error) => Err(error),
    }
}

pub fn run(config_path: PathBuf, database_path: PathBuf) -> Result<(), AppError> {
    let start = load_start(&config_path, &database_path)?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("NEXUS")
            .with_inner_size([1_080.0, 720.0])
            .with_min_inner_size([760.0, 520.0]),
        ..Default::default()
    };

    eframe::run_native(
        "NEXUS",
        options,
        Box::new(move |_creation_context| {
            Ok(Box::new(RootApp::new(start, config_path, database_path)))
        }),
    )?;

    Ok(())
}

/// Owns either the workbench or the configuration recovery screen and switches
/// between them in place, so recovery never needs a second event loop.
struct RootApp {
    config_path: PathBuf,
    database_path: PathBuf,
    ready: Option<Box<NexusApp>>,
    recovering: Option<recovery::RecoveryScreen>,
}

impl RootApp {
    fn new(start: Start, config_path: PathBuf, database_path: PathBuf) -> Self {
        let mut app = Self {
            config_path,
            database_path,
            ready: None,
            recovering: None,
        };
        match start {
            Start::Ready(config, storage) => {
                app.ready = Some(Box::new(NexusApp::new(
                    config,
                    app.config_path.clone(),
                    app.database_path.clone(),
                    storage,
                )));
            }
            Start::Recover(error) => {
                app.recovering = Some(recovery::RecoveryScreen::from_error(
                    app.config_path.clone(),
                    &error,
                ));
            }
        }
        app
    }

    fn enter_workbench(&mut self, config: Config) -> Result<(), String> {
        match Storage::open(&self.database_path) {
            Ok(storage) => {
                self.ready = Some(Box::new(NexusApp::new(
                    config,
                    self.config_path.clone(),
                    self.database_path.clone(),
                    storage,
                )));
                self.recovering = None;
                Ok(())
            }
            Err(error) => Err(format!(
                "The configuration is readable, but the local database could not be opened ({error})."
            )),
        }
    }
}

impl eframe::App for RootApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(app) = &mut self.ready {
            app.show(ui);
            return;
        }
        let Some(screen) = &mut self.recovering else {
            return;
        };
        let outcome = screen.show(ui);
        match outcome {
            recovery::RecoveryOutcome::None => {}
            recovery::RecoveryOutcome::Quit => {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            recovery::RecoveryOutcome::Recovered(config) => {
                match self.enter_workbench(config) {
                    Ok(()) => {}
                    Err(message) => {
                        // The borrow of `screen` ends before this call.
                        if let Some(screen) = &mut self.recovering {
                            screen.runtime_error(message);
                        }
                    }
                }
            }
        }
    }
}

struct NexusApp {
    config: Config,
    config_path: PathBuf,
    database_path: PathBuf,
    storage: Storage,
    selected_section: Section,
    status: Option<String>,
    chat: chat::ChatScreen,
    providers: providers::ProvidersScreen,
    mcp: mcp::McpScreen,
    settings: settings::SettingsScreen,
    usage: usage::UsageScreen,
    last_title: String,
    #[cfg(feature = "telemetry")]
    telemetry: crate::telemetry::TelemetryService,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Section {
    Chat,
    Providers,
    Mcp,
    Usage,
    Resources,
    Settings,
}

impl NexusApp {
    fn new(config: Config, config_path: PathBuf, database_path: PathBuf, storage: Storage) -> Self {
        storage.set_history_writes(config.history_enabled);
        storage.set_tool_output_writes(config.save_tool_outputs);
        let _ = storage.prune(config.history_retention_days, DEFAULT_MAX_CONVERSATIONS);
        let chat = chat::ChatScreen::new(&config, &storage);
        let providers = providers::ProvidersScreen::new(&config);
        let mcp = mcp::McpScreen::new(&config);
        Self {
            config,
            config_path,
            database_path,
            storage,
            selected_section: Section::Chat,
            status: None,
            chat,
            providers,
            mcp,
            settings: settings::SettingsScreen::new(),
            usage: usage::UsageScreen::new(),
            last_title: "NEXUS".to_owned(),
            #[cfg(feature = "telemetry")]
            telemetry: crate::telemetry::TelemetryService::start(),
        }
    }

    #[cfg(test)]
    fn headless(
        config: Config,
        config_path: PathBuf,
        database_path: PathBuf,
        storage: Storage,
    ) -> Self {
        storage.set_history_writes(config.history_enabled);
        storage.set_tool_output_writes(config.save_tool_outputs);
        let chat = chat::ChatScreen::new(&config, &storage);
        let providers = providers::ProvidersScreen::new(&config);
        let mcp = mcp::McpScreen::new(&config);
        Self {
            config,
            config_path,
            database_path,
            storage,
            selected_section: Section::Chat,
            status: None,
            chat,
            providers,
            mcp,
            settings: settings::SettingsScreen::new(),
            usage: usage::UsageScreen::new(),
            last_title: "NEXUS".to_owned(),
            #[cfg(feature = "telemetry")]
            telemetry: crate::telemetry::TelemetryService::inert(),
        }
    }

    fn save_config(&mut self) {
        self.status = Some(match self.config.save(&self.config_path) {
            Ok(()) => "Configuration saved.".to_owned(),
            Err(error) => format!("Could not save configuration: {error}"),
        });
    }

    fn update_window_title(&mut self, ctx: &egui::Context) {
        let title = self
            .chat
            .window_title()
            .unwrap_or_else(|| "NEXUS".to_owned());
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }
}

impl NexusApp {
    fn show(&mut self, ui: &mut egui::Ui) {
        settings::apply_config_theme(ui.ctx(), &self.config);
        self.storage.set_history_writes(self.config.history_enabled);
        self.storage
            .set_tool_output_writes(self.config.save_tool_outputs);

        egui::Panel::left("navigation")
            .resizable(false)
            .default_size(210.0)
            .show(ui, |ui| {
                ui.add_space(14.0);
                ui.heading("NEXUS");
                ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                ui.small("API workbench");
                ui.add_space(28.0);
                for (section, label) in [
                    (Section::Chat, "Chat"),
                    (Section::Providers, "Providers"),
                    (Section::Mcp, "MCP"),
                    (Section::Usage, "Usage"),
                    (Section::Resources, "Resources"),
                    (Section::Settings, "Settings"),
                ] {
                    ui.selectable_value(&mut self.selected_section, section, label);
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.separator();
                    ui.small("Local desktop application");
                    if let Some(status) = &self.status {
                        ui.small(status);
                    }
                });
            });

        match self.selected_section {
            Section::Chat => {
                egui::CentralPanel::default().show(ui, |ui| {
                    let outcome = self.chat.show(ui, &mut self.config, &self.storage);
                    if outcome.open_providers {
                        self.selected_section = Section::Providers;
                    }
                    if outcome.dismiss_callout {
                        self.config.first_run_callout_dismissed = true;
                        self.save_config();
                    }
                });
            }
            Section::Providers => {
                egui::CentralPanel::default().show(ui, |ui| {
                    self.providers.show(ui, &mut self.config, &self.config_path);
                });
            }
            Section::Usage => {
                egui::CentralPanel::default().show(ui, |ui| {
                    let outcome = self.usage.show(ui, &self.config, &self.storage);
                    if outcome.open_settings {
                        self.selected_section = Section::Settings;
                    }
                });
            }
            Section::Settings => {
                egui::CentralPanel::default().show(ui, |ui| {
                    let outcome = self.settings.show(
                        ui,
                        &mut self.config,
                        &self.storage,
                        &self.config_path,
                        &self.database_path,
                    );
                    if outcome.history_cleared {
                        self.chat.on_history_cleared();
                    }
                });
            }
            Section::Mcp => {
                egui::CentralPanel::default().show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.add_space(12.0);
                            self.mcp.show(ui, &mut self.config, &self.config_path);
                        });
                });
            }
            Section::Resources => {
                egui::CentralPanel::default().show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.add_space(12.0);
                            #[cfg(feature = "telemetry")]
                            show_resources(ui, &self.telemetry);
                            #[cfg(not(feature = "telemetry"))]
                            {
                                ui.heading("Resources");
                                ui.label("Telemetry is disabled in this build.");
                            }
                        });
                });
            }
        }
        self.update_window_title(ui.ctx());
    }
}

#[cfg(feature = "telemetry")]
fn show_resources(ui: &mut egui::Ui, telemetry: &crate::telemetry::TelemetryService) {
    use crate::telemetry::CollectionStatus;
    use std::time::Duration;

    ui.ctx().request_repaint_after(Duration::from_secs(2));
    let snapshot = telemetry.snapshot();
    ui.heading("Resource Monitor");
    ui.label("NEXUS process only · RSS, not macOS physical footprint");
    egui::Grid::new("resource_metrics")
        .num_columns(2)
        .spacing([24.0, 8.0])
        .show(ui, |ui| {
            ui.label("Status");
            ui.strong(match snapshot.status {
                CollectionStatus::Starting => "Starting",
                CollectionStatus::Running => "Monitoring",
                CollectionStatus::Unavailable => "Unavailable",
            });
            ui.end_row();
            ui.label("PID");
            ui.strong(snapshot.pid.to_string());
            ui.end_row();
            ui.label("Uptime");
            ui.strong(format_duration(snapshot.uptime));
            ui.end_row();
            ui.label("Current RSS");
            ui.strong(
                snapshot
                    .latest
                    .as_ref()
                    .map(|sample| format!("{:.1} MB", sample.rss_bytes as f64 / 1_000_000.0))
                    .unwrap_or_else(|| "Unavailable".to_owned()),
            );
            ui.end_row();
            ui.label("Peak observed RSS");
            ui.strong(
                snapshot
                    .peak_rss_bytes
                    .map(|bytes| format!("{:.1} MB", bytes as f64 / 1_000_000.0))
                    .unwrap_or_else(|| "Unavailable".to_owned()),
            );
            ui.end_row();
            ui.label("CPU");
            ui.strong(
                snapshot
                    .latest
                    .as_ref()
                    .and_then(|sample| sample.cpu_percent)
                    .map(|value| format!("{value:.1}%"))
                    .unwrap_or_else(|| "Warming up".to_owned()),
            );
            ui.end_row();
            ui.label("Samples retained");
            ui.strong(snapshot.history.len().to_string());
            ui.end_row();
        });
    if let Some(error) = &snapshot.last_error {
        ui.colored_label(ui.visuals().warn_fg_color, error);
    }
}

#[cfg(feature = "telemetry")]
fn format_duration(duration: std::time::Duration) -> String {
    let seconds = duration.as_secs();
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3_600,
        (seconds % 3_600) / 60,
        seconds % 60
    )
}
