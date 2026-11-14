use std::path::PathBuf;

use eframe::egui;

use crate::config::Config;
use crate::error::AppError;
use crate::storage::{DEFAULT_MAX_CONVERSATIONS, Storage};

mod chat;
mod providers;
mod settings;
mod usage;

pub fn run(
    config: Config,
    config_path: PathBuf,
    database_path: PathBuf,
    storage: Storage,
) -> Result<(), AppError> {
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
            Ok(Box::new(NexusApp::new(
                config,
                config_path,
                database_path,
                storage,
            )))
        }),
    )?;

    Ok(())
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
        Self {
            config,
            config_path,
            database_path,
            storage,
            selected_section: Section::Chat,
            status: None,
            chat,
            providers,
            settings: settings::SettingsScreen::new(),
            usage: usage::UsageScreen::new(),
            last_title: "NEXUS".to_owned(),
            #[cfg(feature = "telemetry")]
            telemetry: crate::telemetry::TelemetryService::start(),
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

impl eframe::App for NexusApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
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
            Section::Mcp | Section::Resources => {
                egui::CentralPanel::default().show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.add_space(12.0);
                            match self.selected_section {
                                Section::Mcp => show_mcp_placeholder(ui),
                                Section::Resources => {
                                    #[cfg(feature = "telemetry")]
                                    show_resources(ui, &self.telemetry);
                                    #[cfg(not(feature = "telemetry"))]
                                    {
                                        ui.heading("Resources");
                                        ui.label("Telemetry is disabled in this build.");
                                    }
                                }
                                _ => {}
                            }
                        });
                });
            }
        }
        self.update_window_title(ui.ctx());
    }
}

fn show_mcp_placeholder(ui: &mut egui::Ui) {
    ui.heading("MCP");
    ui.label("MCP client connections will appear here. No server is started automatically.");
    if ui
        .small_button("Learn about the planned MCP boundary")
        .clicked()
    {
        ui.ctx()
            .open_url(egui::OpenUrl::new_tab("https://modelcontextprotocol.io"));
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
