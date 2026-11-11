use std::path::PathBuf;
#[cfg(feature = "telemetry")]
use std::time::Duration;

use eframe::egui;

use crate::agent::{AgentId, AgentRecord, IntegrationStatus, Knowledge};
use crate::config::Config;
use crate::discovery::{
    DiscoveryController, DiscoveryScope, EXPANDED_SCOPE_VERSION, ScanOutcome, ScanState,
    standard_search_plan, validate_custom_location,
};
use crate::error::AppError;
use crate::registry::AgentRegistry;

pub fn run(config: Config, config_path: PathBuf, registry_path: PathBuf) -> Result<(), AppError> {
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
        Box::new(move |_creation_context| {
            Ok(Box::new(NexusApp::new(config, config_path, registry_path)))
        }),
    )?;

    Ok(())
}

struct NexusApp {
    config: Config,
    config_path: PathBuf,
    config_status: Option<String>,
    selected_section: Section,
    registry: AgentRegistry,
    registry_path: PathBuf,
    registry_status: Option<String>,
    discovery: DiscoveryController,
    agent_ui: AgentUiState,
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

#[derive(Default)]
struct AgentUiState {
    expanded_consent_open: bool,
    custom_location_message: Option<String>,
    version_editor: Option<AgentId>,
    version_value: String,
}

impl NexusApp {
    fn new(config: Config, config_path: PathBuf, registry_path: PathBuf) -> Self {
        let (registry, registry_status) = match AgentRegistry::load_or_default(&registry_path) {
            Ok(registry) => (registry, None),
            Err(error) => {
                tracing::warn!("Agent registry could not be loaded: {error}");
                (
                    AgentRegistry::empty(),
                    Some(format!("Agent registry could not be loaded: {error}")),
                )
            }
        };
        Self {
            config,
            config_path,
            config_status: None,
            selected_section: Section::Overview,
            registry,
            registry_path,
            registry_status,
            discovery: DiscoveryController::default(),
            agent_ui: AgentUiState::default(),
            #[cfg(feature = "telemetry")]
            telemetry: crate::telemetry::TelemetryService::start(),
        }
    }
}

impl eframe::App for NexusApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(outcome) = self.discovery.poll() {
            match outcome {
                ScanOutcome::Complete(report) => {
                    self.registry.merge_scan(report);
                    self.registry_status =
                        Some(match self.registry.save_atomically(&self.registry_path) {
                            Ok(()) => "Discovery completed and registry saved.".to_owned(),
                            Err(error) => {
                                format!("Discovery completed; registry was not saved: {error}")
                            }
                        });
                }
                ScanOutcome::Cancelled => {
                    self.registry_status =
                        Some("Discovery cancelled; previous registry retained.".to_owned());
                }
            }
        }

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
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add_space(12.0);
                    match self.selected_section {
                        Section::Overview => {
                            ui.heading("Welcome to NEXUS");
                            ui.label("Welcome message");
                            ui.text_edit_singleline(&mut self.config.welcome_message);
                            if ui.button("Save configuration").clicked() {
                                self.config_status =
                                    Some(match self.config.save(&self.config_path) {
                                        Ok(()) => "Configuration saved.".to_owned(),
                                        Err(error) => {
                                            format!("Could not save configuration: {error}")
                                        }
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
                            show_agent_registry(
                                ui,
                                &mut self.discovery,
                                &mut self.registry,
                                &self.registry_path,
                                &mut self.registry_status,
                                &mut self.agent_ui,
                            );
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
        });
    }
}

fn show_agent_registry(
    ui: &mut egui::Ui,
    discovery: &mut DiscoveryController,
    registry: &mut AgentRegistry,
    registry_path: &std::path::Path,
    status: &mut Option<String>,
    agent_ui: &mut AgentUiState,
) {
    let AgentUiState {
        version_editor,
        version_value,
        expanded_consent_open,
        custom_location_message,
    } = agent_ui;
    if matches!(
        discovery.state(),
        ScanState::Scanning | ScanState::Cancelling
    ) {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }

    ui.heading("Agent Registry");
    ui.label("Detection is passive. NEXUS does not execute agents, verify versions, inspect credentials, or establish usability.");
    ui.horizontal(|ui| {
        ui.label("Discovery scope:");
        if ui
            .selectable_label(
                registry.discovery_scope == DiscoveryScope::Standard,
                "Standard",
            )
            .clicked()
        {
            registry.discovery_scope = DiscoveryScope::Standard;
            registry.expanded_scope_approval_version = None;
            save_registry(registry, registry_path, status);
        }
        if ui
            .selectable_label(
                registry.discovery_scope == DiscoveryScope::Expanded,
                "Expanded system discovery",
            )
            .clicked()
            && (registry.discovery_scope != DiscoveryScope::Expanded
                || registry.expanded_scope_approval_version != Some(EXPANDED_SCOPE_VERSION))
        {
            *expanded_consent_open = true;
        }
    });
    ui.small(match registry.discovery_scope {
        DiscoveryScope::Standard => "Standard checks documented conventional local directories and eligible PATH entries.",
        DiscoveryScope::Expanded => "Expanded recursively checks only the locations listed in its approval dialog, within strict limits.",
    });
    ui.horizontal(|ui| {
        let scope_approved = registry.discovery_scope == DiscoveryScope::Standard
            || registry.expanded_scope_approval_version == Some(EXPANDED_SCOPE_VERSION);
        if ui
            .add_enabled(scope_approved, egui::Button::new("Rescan candidates"))
            .clicked()
            && matches!(discovery.state(), ScanState::Idle | ScanState::Failed(_))
        {
            *status = Some("Scanning the selected bounded local search scope…".to_owned());
            let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
            let plan = standard_search_plan(
                home.as_deref(),
                std::env::var_os("PATH").as_deref(),
                &registry.custom_search_locations,
            );
            discovery.start(plan, registry.discovery_scope);
        }
        if ui
            .add_enabled(
                matches!(discovery.state(), ScanState::Scanning),
                egui::Button::new("Cancel scan"),
            )
            .clicked()
        {
            discovery.cancel();
        }
        ui.label(match discovery.state() {
            ScanState::Idle => "Idle",
            ScanState::Scanning => "Scanning",
            ScanState::Cancelling => "Cancelling after the current filesystem check",
            ScanState::Failed(_) => "Scan unavailable",
        });
    });
    if registry.discovery_scope == DiscoveryScope::Expanded
        && registry.expanded_scope_approval_version != Some(EXPANDED_SCOPE_VERSION)
    {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "Expanded discovery needs approval before scanning.",
        );
    }
    if matches!(
        discovery.state(),
        ScanState::Scanning | ScanState::Cancelling
    ) {
        let progress = discovery.progress();
        ui.label(format!(
            "Progress: {} locations checked · {} entries examined",
            progress.locations_checked, progress.entries_examined
        ));
        if let Some(path) = progress.current_location {
            ui.small(format!("Current location: {}", path.display()));
        }
    }

    if let ScanState::Failed(error) = discovery.state() {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    if let Some(status) = status {
        ui.label(status.as_str());
    }

    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !matches!(
                    discovery.state(),
                    ScanState::Scanning | ScanState::Cancelling
                ),
                egui::Button::new("Add Search Location…"),
            )
            .clicked()
        {
            #[cfg(target_os = "macos")]
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                match validate_custom_location(&path) {
                    Ok(path) if registry.add_custom_location(path.clone()) => {
                        save_registry(registry, registry_path, status);
                        *custom_location_message = Some(format!("Added: {}", path.display()));
                    }
                    Ok(_) => {
                        *custom_location_message = Some(
                            "Location already exists or the custom-location limit was reached."
                                .into(),
                        )
                    }
                    Err(reason) => {
                        *custom_location_message = Some(format!("Location rejected: {reason}"))
                    }
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                *custom_location_message =
                    Some("The native folder picker is supported on macOS only.".into());
            }
        }
    });
    if !registry.custom_search_locations.is_empty() || custom_location_message.is_some() {
        ui.collapsing("Custom search locations", |ui| {
            ui.label("Passive, nonrecursive exact-name checks directly inside these folders only. This does not authorize execution or control.");
            for path in registry.custom_search_locations.clone() {
                ui.horizontal(|ui| {
                    ui.label(path.display().to_string());
                    if ui.add_enabled(
                        !matches!(discovery.state(), ScanState::Scanning | ScanState::Cancelling),
                        egui::Button::new("Remove"),
                    ).clicked() {
                        registry.remove_custom_location(&path);
                        save_registry(registry, registry_path, status);
                        *custom_location_message = Some("Removed; subsequent scans will omit this location.".into());
                    }
                });
            }
            if let Some(message) = custom_location_message.as_deref() { ui.small(message); }
        });
    }

    if let Some(last_scan) = registry.last_scan_unix_secs {
        ui.small(format!(
            "Last scan: {} · {} checked locations · {} skipped/inaccessible/excluded locations",
            format_age(last_scan),
            registry.last_search_roots.len(),
            registry.last_skipped_roots.len()
        ));
        ui.collapsing("Search scope details", |ui| {
            for result in &registry.last_location_results {
                let label = match result.status {
                    crate::discovery::LocationStatus::Checked => "Checked",
                    crate::discovery::LocationStatus::Missing => "Missing",
                    crate::discovery::LocationStatus::Skipped => "Skipped",
                    crate::discovery::LocationStatus::Inaccessible => "Inaccessible",
                    crate::discovery::LocationStatus::Excluded => "Excluded",
                };
                ui.small(format!(
                    "{label}: {} — {}",
                    result.path.display(),
                    result.reason
                ));
            }
        });
    }

    ui.separator();
    ui.label("Detected means a candidate executable was found in the displayed search scope. It does not mean NEXUS can control or authenticate with it.");

    let visible_indices: Vec<usize> = registry
        .records
        .iter()
        .enumerate()
        .filter_map(|(index, record)| (!record.ignored).then_some(index))
        .collect();
    for index in visible_indices {
        let id = registry.records[index].id;
        show_agent_card(ui, &mut registry.records[index], version_editor);
        ui.add_space(8.0);

        let mut changed = false;
        ui.horizontal(|ui| {
            if ui.button("Ignore candidate").clicked() {
                changed = true;
            }
        });
        if changed {
            registry.set_ignored(id, true);
            save_registry(registry, registry_path, status);
        }
    }

    let ignored_indices: Vec<usize> = registry
        .records
        .iter()
        .enumerate()
        .filter_map(|(index, record)| record.ignored.then_some(index))
        .collect();
    if !ignored_indices.is_empty() {
        ui.collapsing(
            format!("Ignored candidates ({})", ignored_indices.len()),
            |ui| {
                for index in ignored_indices {
                    let id = registry.records[index].id;
                    let name = registry.records[index].name.clone();
                    let mut restored = false;
                    ui.horizontal(|ui| {
                        ui.label(name);
                        if ui.button("Restore").clicked() {
                            restored = true;
                        }
                    });
                    if restored {
                        registry.set_ignored(id, false);
                        save_registry(registry, registry_path, status);
                    }
                }
            },
        );
    }

    if let Some(agent_id) = *version_editor {
        let version_command = registry
            .records
            .iter()
            .find(|record| record.id == agent_id)
            .and_then(|record| record.locations.first())
            .and_then(|location| {
                agent_id
                    .version_command()
                    .map(|argument| format!("{} {argument}", shell_quote(&location.resolved_path)))
            });
        egui::Window::new(format!("Verify {} version", agent_id.display_name()))
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.label("This is a separate, user-run verification step. NEXUS will not execute the command.");
                if let Some(command) = &version_command {
                    ui.label("Run manually in Terminal:");
                    ui.code(command);
                } else {
                    ui.label("No version command is currently documented for this candidate.");
                }
                ui.label("Enter the version reported by your manual check:");
                ui.text_edit_singleline(version_value);
                ui.horizontal(|ui| {
                    if ui.button("Record user-reported version").clicked()
                        && !version_value.trim().is_empty()
                    {
                        registry.record_user_reported_version(
                            agent_id,
                            version_value.trim().chars().take(128).collect(),
                            crate::discovery::unix_time_now(),
                        );
                        *version_editor = None;
                        version_value.clear();
                        save_registry(registry, registry_path, status);
                    }
                    if ui.button("Close").clicked() {
                        *version_editor = None;
                        version_value.clear();
                    }
                });
            });
    }

    if *expanded_consent_open {
        egui::Window::new("Approve Expanded System Discovery")
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.label("Expanded discovery searches directory names and executable metadata only. It never reads file contents or launches anything.");
                ui.label("Roots: /Applications, /opt, and your home Applications directory.");
                ui.label("Limits: depth 4; at most 128 directories and 3,000 entries; 10 seconds; cancellable.");
                ui.label("System/Library, credentials, configuration stores, personal files, known network volumes, and symlinked directories are excluded.");
                ui.label("The scan makes no network requests and does not enumerate processes. Permission covers passive detection only.");
                ui.horizontal(|ui| {
                    if ui.button("Approve and enable Expanded Discovery").clicked() {
                        registry.discovery_scope = DiscoveryScope::Expanded;
                        registry.expanded_scope_approval_version = Some(EXPANDED_SCOPE_VERSION);
                        *expanded_consent_open = false;
                        save_registry(registry, registry_path, status);
                    }
                    if ui.button("Decline").clicked() {
                        registry.discovery_scope = DiscoveryScope::Standard;
                        registry.expanded_scope_approval_version = None;
                        *expanded_consent_open = false;
                    }
                });
            });
    }
}

fn show_agent_card(
    ui: &mut egui::Ui,
    record: &mut AgentRecord,
    version_editor: &mut Option<AgentId>,
) {
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.heading(&record.name);
            ui.label("Agent harness");
            ui.strong(record.detection_status.label());
        });
        ui.label(format!("Executable name: {}", record.executable_name));
        if record.locations.is_empty() {
            ui.label("Executable location: not found or not verified in the current search scope");
        } else {
            for location in &record.locations {
                ui.label(format!("Executable: {}", location.discovered_path.display()));
                if location.discovered_path != location.resolved_path {
                    ui.small(format!("Resolved within authorized root: {}", location.resolved_path.display()));
                }
            }
        }
        ui.horizontal(|ui| {
            ui.label("Installed version:");
            match &record.installed_version {
                Knowledge::UserReported { value, .. } => {
                    ui.label(format!("{value} (user-reported; not independently verified)"));
                }
                Knowledge::Unknown => {
                    ui.label("Unknown");
                }
                Knowledge::Documented { value, .. } => {
                    ui.label(format!("{value} (documentation claim)"));
                }
                Knowledge::Observed { value, .. } => {
                    ui.label(format!("{value} (observed)"));
                }
                Knowledge::Inferred { value, .. } => {
                    ui.label(format!("{value} (inferred)"));
                }
            }
            if !record.locations.is_empty() && ui.button("Verify version manually…").clicked() {
                *version_editor = Some(record.id);
            }
        });

        ui.label("Running: not checked · Authenticated: not checked · Connectable: no adapter · Usable: no adapter");
        ui.label(match record.integration_status {
            IntegrationStatus::DiscoveryOnly => "NEXUS integration: Discovery only",
            IntegrationStatus::AdapterNotImplemented => "NEXUS integration: Adapter not implemented",
        });

        ui.label("Documented interfaces (not verified for this installation):");
        for claim in &record.interfaces {
            match claim {
                Knowledge::Documented { value, .. } => {
                    ui.label(format!("• {} · vendor documented", value.label()));
                }
                Knowledge::Inferred { value, reason } => {
                    ui.label(format!("• {} · inferred: {reason}", value.label()));
                }
                Knowledge::Observed { value, .. } => {
                    ui.label(format!("• {} · observed", value.label()));
                }
                Knowledge::UserReported { value, .. } => {
                    ui.label(format!("• {} · user reported", value.label()));
                }
                Knowledge::Unknown => {
                    ui.label("• Interface unknown");
                }
            }
        }
        ui.label("Installed-version capabilities:");
        for claim in &record.capabilities {
            match claim {
                Knowledge::Documented { value, .. } => {
                    ui.label(format!("• {value} · vendor documented"));
                }
                Knowledge::Observed { value, .. } => {
                    ui.label(format!("• {value} · observed"));
                }
                Knowledge::Inferred { value, reason } => {
                    ui.label(format!("• {value} · inferred: {reason}"));
                }
                Knowledge::UserReported { value, .. } => {
                    ui.label(format!("• {value} · user reported"));
                }
                Knowledge::Unknown => {
                    ui.label("• Unknown until checked for the installed version");
                }
            }
        }
        ui.small(format!("Documentation: {}", record.id.documentation_url()));
    });
}

fn save_registry(registry: &AgentRegistry, path: &std::path::Path, status: &mut Option<String>) {
    *status = Some(match registry.save_atomically(path) {
        Ok(()) => "Registry updated.".to_owned(),
        Err(error) => format!("Registry change could not be saved: {error}"),
    });
}

fn shell_quote(path: &std::path::Path) -> String {
    let value = path.display().to_string();
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn format_age(timestamp: u64) -> String {
    let age = crate::discovery::unix_time_now().saturating_sub(timestamp);
    if age < 60 {
        format!("{age}s ago")
    } else if age < 3_600 {
        format!("{}m ago", age / 60)
    } else if age < 86_400 {
        format!("{}h ago", age / 3_600)
    } else {
        format!("{}d ago", age / 86_400)
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
