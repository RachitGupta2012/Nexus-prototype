use std::path::PathBuf;
#[cfg(feature = "telemetry")]
use std::time::Duration;

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
    #[cfg(feature = "telemetry")]
    telemetry: crate::telemetry::TelemetryService,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Overview,
    Agents,
    Missions,
    Resources,
}

impl NexusApp {
    fn new(config: Config, config_path: PathBuf) -> Self {
        Self {
            config,
            config_path,
            config_status: None,
            selected_section: Section::Overview,
            #[cfg(feature = "telemetry")]
            telemetry: crate::telemetry::TelemetryService::start(),
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
                ui.selectable_value(&mut self.selected_section, Section::Resources, "Resources");

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
                Section::Resources => {
                    #[cfg(feature = "telemetry")]
                    show_resource_monitor(ui, &self.telemetry);

                    #[cfg(not(feature = "telemetry"))]
                    show_telemetry_disabled(ui);
                }
            }
        });
    }
}

#[cfg(feature = "telemetry")]
fn show_resource_monitor(ui: &mut egui::Ui, telemetry: &crate::telemetry::TelemetryService) {
    use crate::telemetry::CollectionStatus;

    ui.ctx().request_repaint_after(Duration::from_secs(2));
    let snapshot = telemetry.snapshot();

    ui.heading("Resource Monitor");
    ui.label("NEXUS process only · RAM is RSS · CPU is relative to one logical core");

    egui::Grid::new("resource_metrics")
        .num_columns(3)
        .spacing([28.0, 10.0])
        .show(ui, |ui| {
            ui.label("Application status");
            ui.strong("Running");
            ui.label(match snapshot.status {
                CollectionStatus::Starting => "Starting metrics",
                CollectionStatus::Running => "Monitoring active",
                CollectionStatus::Unavailable => "Metrics unavailable",
            });
            ui.end_row();

            ui.label("PID");
            ui.strong(snapshot.pid.to_string());
            ui.label("NEXUS only");
            ui.end_row();

            ui.label("Uptime");
            ui.strong(format_duration(snapshot.uptime));
            ui.label("Since application start");
            ui.end_row();

            ui.label(if snapshot.status == CollectionStatus::Unavailable {
                "Last RSS"
            } else {
                "Current RSS"
            });
            ui.strong(
                snapshot
                    .latest
                    .as_ref()
                    .map(|sample| format_megabytes(sample.rss_bytes))
                    .unwrap_or_else(|| "Unavailable".to_owned()),
            );
            ui.label("Resident set size");
            ui.end_row();

            ui.label("Peak observed RSS");
            ui.strong(
                snapshot
                    .peak_rss_bytes
                    .map(format_megabytes)
                    .unwrap_or_else(|| "Unavailable".to_owned()),
            );
            ui.label("This application session");
            ui.end_row();

            ui.label("CPU");
            ui.strong(
                snapshot
                    .latest
                    .as_ref()
                    .and_then(|sample| sample.cpu_percent)
                    .map(|percent| format!("{percent:.1}%"))
                    .unwrap_or_else(|| "Warming up".to_owned()),
            );
            ui.label("One logical core = 100%");
            ui.end_row();
        });

    if let Some(error) = &snapshot.last_error {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!("Latest sample unavailable: {error}"),
        );
    }
    if let Some(sample) = &snapshot.latest {
        ui.label(format!(
            "Last successful sample: {} ago",
            format_duration(snapshot.uptime.saturating_sub(sample.elapsed))
        ));
        show_memory_status(ui, sample.rss_bytes);
    }

    ui.add_space(12.0);
    ui.heading("RSS history · last 10 minutes");
    show_rss_history(ui, &snapshot.history);
}

#[cfg(feature = "telemetry")]
fn show_memory_status(ui: &mut egui::Ui, rss_bytes: u64) {
    let rss_mb = rss_bytes as f64 / 1_000_000.0;
    let (label, color) = if rss_mb >= 1_000.0 {
        (
            "Absolute 1,000 MB goal exceeded",
            ui.visuals().error_fg_color,
        )
    } else if rss_mb >= 900.0 {
        ("Critical · 900 MB threshold", ui.visuals().error_fg_color)
    } else if rss_mb >= 700.0 {
        ("Warning · 700 MB threshold", ui.visuals().warn_fg_color)
    } else {
        ("Below warning threshold", ui.visuals().text_color())
    };
    ui.colored_label(color, label);
}

#[cfg(feature = "telemetry")]
fn show_rss_history(
    ui: &mut egui::Ui,
    history: &std::collections::VecDeque<crate::telemetry::ResourceSample>,
) {
    use eframe::egui::{Color32, Sense, Stroke, Vec2, pos2};

    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 150.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
    let plot = rect.shrink2(Vec2::new(12.0, 10.0));
    let warning_y = plot.bottom() - plot.height() * 700.0 / 1_000.0;
    let critical_y = plot.bottom() - plot.height() * 900.0 / 1_000.0;
    painter.line_segment(
        [pos2(plot.left(), warning_y), pos2(plot.right(), warning_y)],
        Stroke::new(1.0, ui.visuals().warn_fg_color),
    );
    painter.line_segment(
        [
            pos2(plot.left(), critical_y),
            pos2(plot.right(), critical_y),
        ],
        Stroke::new(1.0, ui.visuals().error_fg_color),
    );

    if history.len() > 1 {
        let last_index = (history.len() - 1) as f32;
        let points = history
            .iter()
            .enumerate()
            .map(|(index, sample)| {
                let x = plot.left() + plot.width() * index as f32 / last_index;
                let rss_mb = sample.rss_bytes as f32 / 1_000_000.0;
                let y = plot.bottom() - plot.height() * (rss_mb / 1_000.0).clamp(0.0, 1.0);
                pos2(x, y)
            })
            .collect();
        painter.add(egui::Shape::line(
            points,
            Stroke::new(2.0, Color32::from_rgb(75, 180, 245)),
        ));
    } else {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Waiting for samples…",
            egui::FontId::default(),
            ui.visuals().text_color(),
        );
    }
}

#[cfg(not(feature = "telemetry"))]
fn show_telemetry_disabled(ui: &mut egui::Ui) {
    ui.heading("Resource Monitor");
    ui.label("Telemetry is disabled in this build. NEXUS is running normally.");
}

#[cfg(feature = "telemetry")]
fn format_megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

#[cfg(feature = "telemetry")]
fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}
