use std::path::PathBuf;

use eframe::egui;

use crate::config::Config;
use crate::error::AppError;
use crate::storage::Storage;

mod chat;
mod providers;

pub fn run(config: Config, config_path: PathBuf, storage: Storage) -> Result<(), AppError> {
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
            Ok(Box::new(NexusApp::new(config, config_path, storage)))
        }),
    )?;

    Ok(())
}

struct NexusApp {
    config: Config,
    config_path: PathBuf,
    storage: Storage,
    selected_section: Section,
    status: Option<String>,
    chat: chat::ChatScreen,
    providers: providers::ProvidersScreen,
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
    fn new(config: Config, config_path: PathBuf, storage: Storage) -> Self {
        let chat = chat::ChatScreen::new(&config, &storage);
        let providers = providers::ProvidersScreen::new(&config);
        Self {
            config,
            config_path,
            storage,
            selected_section: Section::Chat,
            status: None,
            chat,
            providers,
            #[cfg(feature = "telemetry")]
            telemetry: crate::telemetry::TelemetryService::start(),
        }
    }
}

impl eframe::App for NexusApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
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
                });
            });

        egui::CentralPanel::default().show(ui, |ui| {
            if self.selected_section == Section::Chat {
                self.chat.show(ui, &self.config, &self.storage);
                return;
            }
            if self.selected_section == Section::Providers {
                self.providers.show(ui, &mut self.config, &self.config_path);
                return;
            }
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add_space(12.0);
                    match self.selected_section {
                        Section::Chat => {}
                        Section::Providers => {}
                        Section::Mcp => {
                            ui.heading("MCP");
                            ui.label("MCP client connections will appear here. No server is started automatically.");
                        }
                        Section::Usage => {
                            ui.heading("Usage");
                            show_usage(ui, &self.storage);
                        }
                        Section::Resources => {
                            #[cfg(feature = "telemetry")]
                            show_resources(ui, &self.telemetry);
                            #[cfg(not(feature = "telemetry"))]
                            {
                                ui.heading("Resources");
                                ui.label("Telemetry is disabled in this build.");
                            }
                        }
                        Section::Settings => {
                            ui.heading("Settings");
                            ui.label("History retention, appearance, and local data controls.");
                            ui.small(format!("Configuration: {}", self.config_path.display()));
                            if ui.button("Save configuration").clicked() {
                                self.status = Some(match self.config.save(&self.config_path) {
                                    Ok(()) => "Configuration saved.".to_owned(),
                                    Err(error) => format!("Could not save configuration: {error}"),
                                });
                            }
                            if let Some(status) = &self.status {
                                ui.label(status);
                            }
                        }
                    }
                });
        });
    }
}

fn show_usage(ui: &mut egui::Ui, storage: &Storage) {
    match storage.usage_rows() {
        Ok(rows) if rows.is_empty() => {
            ui.label("No provider usage has been recorded yet.");
        }
        Ok(rows) => {
            egui::Grid::new("usage_rows")
                .striped(true)
                .num_columns(6)
                .show(ui, |ui| {
                    ui.strong("Provider");
                    ui.strong("Model");
                    ui.strong("Messages");
                    ui.strong("Input tokens");
                    ui.strong("Output tokens");
                    ui.strong("Estimated cost");
                    ui.end_row();
                    for row in rows {
                        ui.label(row.provider_id.to_string());
                        ui.label(&row.model);
                        ui.label(row.message_count.to_string());
                        ui.label(row.input_tokens.to_string());
                        ui.label(row.output_tokens.to_string());
                        ui.label(match row.currency {
                            Some(currency) if row.estimated_cost > 0.0 => {
                                format!("~${:.4} {currency}", row.estimated_cost)
                            }
                            _ => "—".to_owned(),
                        });
                        ui.end_row();
                    }
                });
        }
        Err(error) => {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("Could not read usage: {error}"),
            );
        }
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
