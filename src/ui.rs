use std::path::PathBuf;

use eframe::egui;

use crate::config::Config;
use crate::error::AppError;

pub fn run(config: Config, config_path: PathBuf) -> Result<(), AppError> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("NEXUS")
            .with_inner_size([960.0, 640.0])
            .with_min_inner_size([640.0, 420.0]),
        ..Default::default()
    };

    eframe::run_native(
        "NEXUS",
        options,
        Box::new(move |_creation_context| Ok(Box::new(NexusApp::new(config, config_path)))),
    )?;

    Ok(())
}

struct NexusApp {
    config: Config,
    config_path: PathBuf,
    config_status: Option<String>,
    selected_section: Section,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Overview,
    Agents,
    Missions,
}

impl NexusApp {
    fn new(config: Config, config_path: PathBuf) -> Self {
        Self {
            config,
            config_path,
            config_status: None,
            selected_section: Section::Overview,
        }
    }
}

impl eframe::App for NexusApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::left("navigation")
            .resizable(false)
            .default_size(190.0)
            .show(ui, |ui| {
                ui.add_space(12.0);
                ui.heading("NEXUS");
                ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                ui.add_space(28.0);

                ui.selectable_value(&mut self.selected_section, Section::Overview, "Overview");
                ui.selectable_value(&mut self.selected_section, Section::Agents, "Agents");
                ui.selectable_value(&mut self.selected_section, Section::Missions, "Missions");

                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.separator();
                    ui.small("Local desktop prototype");
                });
            });

        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(12.0);
            match self.selected_section {
                Section::Overview => {
                    ui.heading("Welcome to NEXUS");
                    ui.label("Welcome message");
                    ui.text_edit_singleline(&mut self.config.welcome_message);
                    if ui.button("Save configuration").clicked() {
                        self.config_status = Some(match self.config.save(&self.config_path) {
                            Ok(()) => "Configuration saved.".to_owned(),
                            Err(error) => format!("Could not save configuration: {error}"),
                        });
                    }
                    if let Some(status) = &self.config_status {
                        ui.label(status);
                    }
                    ui.add_space(24.0);
                    ui.group(|ui| {
                        ui.heading("Application status");
                        ui.label("Foundation ready");
                        ui.label("Integrations are not configured in this phase.");
                    });
                }
                Section::Agents => {
                    ui.heading("Agents");
                    ui.label("Agent discovery is planned for a later phase.");
                }
                Section::Missions => {
                    ui.heading("Missions");
                    ui.label("Mission orchestration is planned for a later phase.");
                }
            }
        });
    }
}
