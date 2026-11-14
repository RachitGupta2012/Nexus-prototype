use std::path::Path;

use eframe::egui::{self, TextEdit};

use crate::config::{Config, McpServerConfig, McpTransport};
use crate::mcp::{McpManager, ServerState, config as validation};

pub struct McpScreen {
    manager: McpManager,
    selected: Option<u64>,
    draft: Option<Draft>,
    confirming_remove: bool,
    message: Option<String>,
}

struct Draft {
    server: McpServerConfig,
    is_new: bool,
    args_text: String,
    env_text: String,
}

#[derive(Clone, Copy)]
enum Action {
    Add,
    Connect(u64),
    Disconnect(u64),
    Refresh(u64),
    Edit(u64),
    Remove(u64),
    CancelRemove,
}

impl McpScreen {
    pub fn new(config: &Config) -> Self {
        let mut manager = McpManager::new();
        manager.sync(&config.mcp_servers);
        Self {
            manager,
            selected: config.mcp_servers.first().map(|server| server.id),
            draft: None,
            confirming_remove: false,
            message: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn message_for_test(&self) -> Option<&str> {
        self.message.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn manager_status_for_test(&self, id: u64) -> Option<crate::mcp::ServerState> {
        self.manager.status(id).map(|status| status.state)
    }

    pub fn show(&mut self, ui: &mut egui::Ui, config: &mut Config, config_path: &Path) {
        self.manager.sync(&config.mcp_servers);
        if self.manager.poll() {
            ui.ctx().request_repaint();
        }
        if self.manager.is_busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(200));
        }

        ui.heading("MCP");
        ui.label(
            "Explicit, read-only MCP client. NEXUS connects only when you ask, lists tool metadata, \
             and never executes tools, reads resources, or retrieves prompts.",
        );
        ui.add_space(6.0);

        if self.draft.is_some() {
            let mut draft = self.draft.take().expect("checked above");
            if self.show_editor(ui, config, config_path, &mut draft) {
                self.draft = Some(draft);
            }
            return;
        }

        let mut action = None;
        ui.horizontal(|ui| {
            if ui.button("+ Add server").clicked() {
                action = Some(Action::Add);
            }
            ui.small("Nothing starts automatically. No credentials are stored.");
        });
        ui.separator();

        if config.mcp_servers.is_empty() {
            ui.add_space(12.0);
            ui.label("No MCP servers configured.");
            ui.label("Add a STDIO or Streamable HTTP server to inspect its advertised tools.");
            self.show_message(ui);
        }

        if !config.mcp_servers.is_empty()
            && self
                .selected
                .is_none_or(|id| !config.mcp_servers.iter().any(|server| server.id == id))
        {
            self.selected = config.mcp_servers.first().map(|server| server.id);
        }

        if !config.mcp_servers.is_empty() {
            ui.columns(2, |columns| {
                columns[0].set_min_width(220.0);
                columns[0].heading("Servers");
                egui::ScrollArea::vertical()
                    .id_salt("mcp_server_list")
                    .max_height(460.0)
                    .auto_shrink([false, false])
                    .show(&mut columns[0], |ui| {
                        for server in &config.mcp_servers {
                            let state = self
                                .manager
                                .status(server.id)
                                .map(|status| status.state)
                                .unwrap_or_default();
                            let tools = self
                                .manager
                                .status(server.id)
                                .map(|status| status.tool_count())
                                .unwrap_or(0);
                            let label = format!(
                                "{}  ·  {}{}",
                                server.name,
                                state.label(),
                                if tools > 0 {
                                    format!("  ·  {tools} tools")
                                } else {
                                    String::new()
                                }
                            );
                            if ui
                                .selectable_label(self.selected == Some(server.id), label)
                                .clicked()
                            {
                                self.selected = Some(server.id);
                                self.confirming_remove = false;
                            }
                        }
                    });

                columns[1].heading("Server detail");
                let Some(id) = self.selected else {
                    columns[1].label("Select a server.");
                    return;
                };
                let Some(server) = config.mcp_servers.iter().find(|server| server.id == id) else {
                    return;
                };
                self.show_detail(&mut columns[1], server, &mut action);
            });
        }

        if let Some(action) = action {
            self.handle(action, config, config_path);
        }
        self.show_message(ui);
    }

    fn show_detail(
        &self,
        ui: &mut egui::Ui,
        server: &McpServerConfig,
        action: &mut Option<Action>,
    ) {
        let status = self.manager.status(server.id);
        let state = status.map(|status| status.state).unwrap_or_default();

        ui.label(egui::RichText::new(&server.name).strong());
        ui.label(match server.transport {
            McpTransport::Stdio => "STDIO process",
            McpTransport::StreamableHttp => "Streamable HTTP",
        });
        if !server.enabled {
            ui.colored_label(ui.visuals().warn_fg_color, "Disabled in configuration");
        }
        ui.add(
            egui::Label::new(
                egui::RichText::new(match server.transport {
                    McpTransport::Stdio => command_preview(server),
                    McpTransport::StreamableHttp => server.url.clone(),
                })
                .monospace(),
            )
            .wrap(),
        );
        ui.horizontal(|ui| {
            let color = match state {
                ServerState::Ready => ui.visuals().hyperlink_color,
                ServerState::Error => ui.visuals().error_fg_color,
                ServerState::Degraded => ui.visuals().warn_fg_color,
                _ => ui.visuals().text_color(),
            };
            ui.colored_label(color, egui::RichText::new(state.label()).strong());
        });

        ui.horizontal_wrapped(|ui| {
            let busy = state.is_busy();
            if ui
                .add_enabled(
                    server.enabled && !busy && !state.is_connected(),
                    egui::Button::new("Connect"),
                )
                .clicked()
            {
                *action = Some(Action::Connect(server.id));
            }
            if ui
                .add_enabled(
                    state.is_connected() && !busy,
                    egui::Button::new("Disconnect"),
                )
                .clicked()
            {
                *action = Some(Action::Disconnect(server.id));
            }
            let can_refresh = matches!(state, ServerState::Ready | ServerState::Degraded);
            if ui
                .add_enabled(can_refresh, egui::Button::new("Refresh metadata"))
                .clicked()
            {
                *action = Some(Action::Refresh(server.id));
            }
            if ui.button("Edit").clicked() {
                *action = Some(Action::Edit(server.id));
            }
            if !self.confirming_remove && ui.button("Remove").clicked() {
                *action = Some(Action::Remove(server.id));
            }
        });

        if let Some(status) = status {
            if let Some(error) = &status.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            if status.stale && !status.tools.is_empty() {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Showing a stale catalog from the last successful discovery.",
                );
            }
            ui.add_space(6.0);
            egui::Grid::new("mcp_identity")
                .num_columns(2)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    ui.label("Protocol");
                    ui.label(
                        status
                            .protocol_version
                            .as_deref()
                            .unwrap_or("Not negotiated"),
                    );
                    ui.end_row();
                    if let Some(name) = &status.server_name {
                        ui.label("Server");
                        ui.label(match &status.server_version {
                            Some(version) => format!("{name} {version}"),
                            None => name.clone(),
                        });
                        ui.end_row();
                    }
                    ui.label("Last discovery");
                    ui.label(
                        status
                            .last_discovery
                            .map(format_time)
                            .unwrap_or_else(|| "Never".to_owned()),
                    );
                    ui.end_row();
                    ui.label("Capabilities");
                    let labels = status.capabilities.labels();
                    ui.label(if labels.is_empty() {
                        "None advertised".to_owned()
                    } else {
                        labels.join(" · ")
                    });
                    ui.end_row();
                });

            ui.add_space(8.0);
            ui.separator();
            ui.heading("Tools");
            if status.last_discovery.is_none() {
                ui.label("Connect to discover this server's tool metadata.");
            } else {
                ui.label("Discovered metadata only. Execution is not implemented in this build.");
                if status.omitted_tools > 0 {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!(
                            "{} tool(s) were omitted because metadata exceeded limits.",
                            status.omitted_tools
                        ),
                    );
                }
                if !status.capabilities.tools {
                    ui.label("The server did not advertise a tools capability.");
                } else if status.tools.is_empty() {
                    ui.label("No tools were advertised.");
                }
                egui::ScrollArea::vertical()
                    .id_salt("mcp_tool_list")
                    .max_height(300.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for tool in &status.tools {
                            ui.collapsing(&tool.name, |ui| {
                                if !tool.description.is_empty() {
                                    ui.label(&tool.description);
                                } else {
                                    ui.weak("No description.");
                                }
                                ui.label(schema_summary(&tool.input_schema));
                                ui.small(tool.status_label());
                            });
                        }
                    });
            }
        }

        if self.confirming_remove && self.selected == Some(server.id) {
            ui.separator();
            ui.group(|ui| {
                ui.label(format!(
                    "Remove “{}”? The connection is closed and the saved settings are deleted.",
                    server.name
                ));
                ui.horizontal(|ui| {
                    if ui.button("Confirm remove").clicked() {
                        *action = Some(Action::Remove(server.id));
                    }
                    if ui.button("Cancel").clicked() {
                        *action = Some(Action::CancelRemove);
                    }
                });
            });
        }
    }

    fn show_editor(
        &mut self,
        ui: &mut egui::Ui,
        config: &mut Config,
        config_path: &Path,
        draft: &mut Draft,
    ) -> bool {
        let title = if draft.is_new {
            "New MCP server"
        } else {
            "Edit MCP server"
        };
        ui.heading(title);
        ui.label("Changes apply to the saved configuration only after you save.");
        ui.add_space(6.0);

        egui::Grid::new("mcp_editor")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Name");
                labelled_text_edit(ui, &mut draft.server.name, 320.0, "Server name");
                ui.end_row();

                ui.label("Transport");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut draft.server.transport, McpTransport::Stdio, "STDIO");
                    ui.selectable_value(
                        &mut draft.server.transport,
                        McpTransport::StreamableHttp,
                        "Streamable HTTP",
                    );
                });
                ui.end_row();

                if draft.server.transport == McpTransport::Stdio {
                    ui.label("Executable");
                    labelled_text_edit(ui, &mut draft.server.command, 320.0, "Executable path");
                    ui.end_row();
                    ui.label("Arguments");
                    ui.add(
                        TextEdit::multiline(&mut draft.args_text)
                            .desired_rows(3)
                            .desired_width(320.0)
                            .hint_text("One argument per line"),
                    )
                    .widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::accesskit::Role::MultilineTextInput,
                            true,
                            "Arguments, one per line",
                        )
                    });
                    ui.end_row();
                    ui.label("Folder");
                    let mut cwd = draft.server.cwd.clone().unwrap_or_default();
                    let response = labelled_text_edit(ui, &mut cwd, 320.0, "Working directory");
                    if response.changed() {
                        draft.server.cwd = (!cwd.trim().is_empty()).then_some(cwd);
                    }
                    ui.end_row();
                    ui.label("Environment allowlist");
                    ui.add(
                        TextEdit::multiline(&mut draft.env_text)
                            .desired_rows(2)
                            .desired_width(320.0)
                            .hint_text("Variable names, one per line"),
                    )
                    .widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::accesskit::Role::MultilineTextInput,
                            true,
                            "Environment variable names, one per line",
                        )
                    });
                    ui.end_row();
                } else {
                    ui.label("Endpoint");
                    labelled_text_edit(ui, &mut draft.server.url, 320.0, "Endpoint URL");
                    ui.end_row();
                }

                ui.label("Enabled");
                ui.checkbox(&mut draft.server.enabled, "Allow explicit connections");
                ui.end_row();
            });

        ui.add_space(6.0);
        let keep = ui.horizontal(|ui| {
            if ui.button("Save configuration").clicked() {
                draft.server.args = split_lines(&draft.args_text);
                draft.server.env_allowlist = split_lines(&draft.env_text);
                if draft.server.transport == McpTransport::StreamableHttp {
                    draft.server.command.clear();
                    draft.server.args.clear();
                    draft.server.cwd = None;
                    draft.server.env_allowlist.clear();
                }
                match validation::validate(&draft.server) {
                    Err(error) => self.message = Some(error.to_owned()),
                    Ok(()) => {
                        if draft.is_new {
                            config.mcp_servers.push(draft.server.clone());
                            config.mcp_sequence = config
                                .mcp_sequence
                                .max(config.mcp_servers.iter().map(|s| s.id).max().unwrap_or(0));
                        } else if let Some(existing) = config
                            .mcp_servers
                            .iter_mut()
                            .find(|server| server.id == draft.server.id)
                        {
                            *existing = draft.server.clone();
                        }
                        self.selected = Some(draft.server.id);
                        self.message = Some(match config.save(config_path) {
                            Ok(()) => "MCP server saved.".to_owned(),
                            Err(error) => format!("Could not save configuration: {error}"),
                        });
                        return false;
                    }
                }
            }
            if ui.button("Cancel").clicked() {
                self.message = None;
                return false;
            }
            true
        });

        let preview = command_preview(&draft.server);
        if draft.server.transport == McpTransport::Stdio && !preview.is_empty() {
            ui.add_space(4.0);
            ui.small("Launch preview (not executed while editing):");
            ui.monospace(preview);
        }
        if let Some(message) = &self.message {
            ui.add_space(4.0);
            ui.colored_label(ui.visuals().warn_fg_color, message);
        }
        keep.inner
    }

    fn handle(&mut self, action: Action, config: &mut Config, config_path: &Path) {
        match action {
            Action::Add => {
                let id = config.allocate_mcp_id();
                self.draft = Some(Draft {
                    server: McpServerConfig {
                        id,
                        name: format!("MCP server {id}"),
                        transport: McpTransport::Stdio,
                        command: String::new(),
                        args: Vec::new(),
                        cwd: None,
                        env_allowlist: Vec::new(),
                        url: String::new(),
                        bearer_auth: false,
                        enabled_tools: Vec::new(),
                        enabled: true,
                    },
                    is_new: true,
                    args_text: String::new(),
                    env_text: String::new(),
                });
                self.message = None;
            }
            Action::Connect(id) => {
                let Some(server) = config
                    .mcp_servers
                    .iter()
                    .find(|server| server.id == id)
                    .cloned()
                else {
                    return;
                };
                self.message = Some(match self.manager.connect(&server) {
                    Ok(()) => "Connecting…".to_owned(),
                    Err(error) => error.to_owned(),
                });
            }
            Action::Disconnect(id) => {
                self.manager.disconnect(id);
                self.message = Some("Disconnecting…".to_owned());
            }
            Action::Refresh(id) => {
                self.message = Some(match self.manager.refresh(id) {
                    Ok(()) => "Refreshing tool metadata…".to_owned(),
                    Err(error) => error.to_owned(),
                });
            }
            Action::Edit(id) => {
                let Some(server) = config
                    .mcp_servers
                    .iter()
                    .find(|server| server.id == id)
                    .cloned()
                else {
                    return;
                };
                self.draft = Some(Draft {
                    args_text: server.args.join("\n"),
                    env_text: server.env_allowlist.join("\n"),
                    server,
                    is_new: false,
                });
                self.confirming_remove = false;
                self.message = None;
            }
            Action::CancelRemove => self.confirming_remove = false,
            Action::Remove(id) => {
                if !self.confirming_remove {
                    self.confirming_remove = true;
                    return;
                }
                self.manager.forget(id);
                config.mcp_servers.retain(|server| server.id != id);
                if config.mcp_servers.is_empty() {
                    self.selected = None;
                } else if let Some(next) = config.mcp_servers.first() {
                    self.selected = Some(next.id);
                }
                self.confirming_remove = false;
                self.message = Some(match config.save(config_path) {
                    Ok(()) => "MCP server removed.".to_owned(),
                    Err(error) => format!("Could not save configuration: {error}"),
                });
            }
        }
    }

    fn show_message(&self, ui: &mut egui::Ui) {
        if let Some(message) = &self.message {
            ui.add_space(4.0);
            ui.small(message);
        }
    }
}

fn command_preview(server: &McpServerConfig) -> String {
    let mut parts = Vec::new();
    if !server.command.trim().is_empty() {
        parts.push(server.command.trim().to_owned());
    }
    parts.extend(
        server
            .args
            .iter()
            .filter(|arg| !arg.trim().is_empty())
            .map(|arg| shell_hint(arg)),
    );
    parts.join(" ")
}

fn shell_hint(arg: &str) -> String {
    if arg.chars().any(char::is_whitespace) {
        format!("“{arg}”")
    } else {
        arg.to_owned()
    }
}

fn split_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

fn labelled_text_edit(
    ui: &mut egui::Ui,
    value: &mut String,
    width: f32,
    label: &str,
) -> egui::Response {
    let response = ui.add(TextEdit::singleline(value).desired_width(width));
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::accesskit::Role::TextInput, response.enabled(), label)
    });
    response
}

fn schema_summary(schema: &serde_json::Value) -> String {
    let Some(object) = schema.as_object() else {
        return "Schema is not a JSON object.".to_owned();
    };
    let kind = object
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("unspecified");
    let required: Vec<&str> = object
        .get("required")
        .and_then(|value| value.as_array())
        .map(|items| items.iter().filter_map(|item| item.as_str()).collect())
        .unwrap_or_default();
    let mut fields = Vec::new();
    if let Some(properties) = object.get("properties").and_then(|value| value.as_object()) {
        for (name, property) in properties.iter().take(16) {
            let field_type = property
                .get("type")
                .and_then(|value| value.as_str())
                .unwrap_or("any");
            let marker = if required.iter().any(|item| *item == name) {
                " (required)"
            } else {
                ""
            };
            fields.push(format!("{name}: {field_type}{marker}"));
        }
        if properties.len() > 16 {
            fields.push(format!("… {} more", properties.len() - 16));
        }
    }
    if fields.is_empty() {
        format!("Input schema type: {kind}")
    } else {
        format!("Input ({kind}): {}", fields.join(", "))
    }
}

fn format_time(seconds: u64) -> String {
    use time::OffsetDateTime;
    let Ok(timestamp) = i64::try_from(seconds) else {
        return "Unknown".to_owned();
    };
    let Ok(time) = OffsetDateTime::from_unix_timestamp(timestamp) else {
        return "Unknown".to_owned();
    };
    time.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "Unknown".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_summary_names_properties_and_required_fields() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string"}, "depth": {"type": "integer"}},
            "required": ["path"]
        });
        let summary = schema_summary(&schema);
        assert!(summary.contains("path: string (required)"));
        assert!(summary.contains("depth: integer"));
    }

    #[test]
    fn schema_summary_handles_non_object_and_missing_type() {
        assert_eq!(
            schema_summary(&serde_json::json!(true)),
            "Schema is not a JSON object."
        );
        assert_eq!(
            schema_summary(&serde_json::json!({})),
            "Input schema type: unspecified"
        );
    }

    #[test]
    fn command_preview_is_inert_and_quotes_only_for_reading() {
        let server = McpServerConfig {
            id: 1,
            name: "Test".to_owned(),
            transport: McpTransport::Stdio,
            command: "node".to_owned(),
            args: vec!["server.js".to_owned(), "two words".to_owned()],
            cwd: None,
            env_allowlist: Vec::new(),
            url: String::new(),
            bearer_auth: false,
            enabled_tools: Vec::new(),
            enabled: true,
        };
        assert_eq!(command_preview(&server), "node server.js “two words”");
    }
}
