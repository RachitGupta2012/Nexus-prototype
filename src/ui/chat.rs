//! Conversation screen: history sidebar, message thread, and composer.

use std::time::Duration;

use eframe::egui::{self, Align, Layout, RichText, TextEdit};
use url::Url;

use crate::chat::controller::{ChatController, ChatTarget};
use crate::chat::{
    ConversationSummary, MAX_TOOL_ARGUMENT_BYTES, MessageRole, MessageStatus, StoredMessage,
};
use crate::config::{AuthMode, Config, Protocol, ProviderProfile};
use crate::storage::{Storage, unix_time_now};

const SIDEBAR_WIDTH: f32 = 245.0;
const MIN_THREAD_HEIGHT: f32 = 160.0;
const COMPOSER_RESERVE: f32 = 132.0;
const MAX_CONVERSATIONS: u32 = 200;
const MAX_SEARCH_CHARS: usize = 200;

#[derive(Default)]
pub struct ChatOutcome {
    pub open_providers: bool,
    pub dismiss_callout: bool,
}

pub struct ChatScreen {
    controller: ChatController,
    draft: String,
    provider_id: Option<u64>,
    model: String,
    conversations: Vec<ConversationSummary>,
    search_query: String,
    last_query: Option<String>,
    renaming: Option<(i64, String)>,
    confirm_clear: bool,
    show_system_prompt: bool,
    pending_delete: Option<i64>,
    list_error: Option<String>,
    callout_dismissed: bool,
}

impl ChatScreen {
    pub fn new(config: &Config, storage: &Storage) -> Self {
        let mut screen = Self {
            controller: ChatController::new(),
            draft: String::new(),
            provider_id: None,
            model: String::new(),
            conversations: Vec::new(),
            search_query: String::new(),
            last_query: None,
            renaming: None,
            confirm_clear: false,
            show_system_prompt: false,
            pending_delete: None,
            list_error: None,
            callout_dismissed: false,
        };
        screen.reset_target(config);
        screen.force_reload(storage);
        screen
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        config: &mut Config,
        storage: &Storage,
    ) -> ChatOutcome {
        let mut outcome = ChatOutcome::default();
        self.sync_target(config);
        if self.controller.is_streaming() {
            self.controller.poll(config, storage);
            ui.ctx().request_repaint_after(Duration::from_millis(40));
            if !self.controller.is_streaming() {
                self.force_reload(storage);
            }
        }
        if self.last_query.as_deref() != Some(self.search_query.trim()) {
            self.force_reload(storage);
        }

        let height = ui.available_height();
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(SIDEBAR_WIDTH, height),
                Layout::top_down(Align::LEFT),
                |ui| self.sidebar(ui, config, storage, &mut outcome),
            );
            ui.separator();
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width().max(320.0), height),
                Layout::top_down(Align::LEFT),
                |ui| self.thread(ui, config, storage, &mut outcome),
            );
        });
        outcome
    }

    #[cfg(test)]
    pub(crate) fn selected_id(&self) -> Option<i64> {
        self.controller
            .conversation
            .as_ref()
            .map(|conversation| conversation.id)
    }

    pub fn window_title(&self) -> Option<String> {
        let conversation = self.controller.conversation.as_ref()?;
        let title = conversation.title.trim();
        if title.is_empty() {
            return None;
        }
        Some(format!("NEXUS — {title}"))
    }

    pub fn on_history_cleared(&mut self) {
        self.controller.start_new();
        self.draft.clear();
        self.renaming = None;
        self.pending_delete = None;
        self.confirm_clear = false;
        self.last_query = None;
    }

    fn reset_target(&mut self, config: &Config) {
        let provider = config
            .default_provider()
            .or_else(|| config.providers.first());
        self.provider_id = provider.map(|provider| provider.id);
        self.model = provider.map(default_model).unwrap_or_default();
    }

    fn sync_target(&mut self, config: &Config) {
        if self
            .provider_id
            .and_then(|id| config.provider(id))
            .is_none()
        {
            self.reset_target(config);
        }
        if self.model.trim().is_empty()
            && let Some(provider) = self.provider_id.and_then(|id| config.provider(id))
        {
            self.model = default_model(provider);
        }
    }

    fn force_reload(&mut self, storage: &Storage) {
        self.last_query = Some(self.search_query.trim().to_owned());
        match storage.search_conversations(&self.search_query, MAX_CONVERSATIONS) {
            Ok(conversations) => {
                self.conversations = conversations;
                self.list_error = None;
            }
            Err(error) => self.list_error = Some(format!("Could not load conversations: {error}")),
        }
    }

    fn sidebar(
        &mut self,
        ui: &mut egui::Ui,
        config: &mut Config,
        storage: &Storage,
        outcome: &mut ChatOutcome,
    ) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.heading("Chat");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .button("+ New")
                    .on_hover_text("Start a new conversation")
                    .clicked()
                {
                    self.controller.start_new();
                    self.draft.clear();
                    self.pending_delete = None;
                    self.renaming = None;
                    self.reset_target(config);
                }
            });
        });
        if !config.history_enabled {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "History is off — this conversation stays in memory.",
            );
        }
        ui.add(
            TextEdit::singleline(&mut self.search_query)
                .hint_text("Search conversations")
                .desired_width(f32::INFINITY)
                .char_limit(MAX_SEARCH_CHARS),
        )
        .widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::accesskit::Role::TextInput,
                true,
                "Search conversations",
            )
        });
        ui.separator();

        let now = unix_time_now();
        let history_enabled = config.history_enabled;
        let mut rows = self.conversations.clone();
        if let Some(conversation) = &self.controller.conversation
            && conversation.id < 0
            && !rows.iter().any(|row| row.id == conversation.id)
        {
            rows.insert(
                0,
                ConversationSummary {
                    id: conversation.id,
                    title: conversation.title.clone(),
                    provider_id: conversation.provider_id,
                    model: conversation.model.clone(),
                    updated_at: conversation.updated_at,
                    message_count: self.controller.messages.len() as u64,
                },
            );
        }
        let mut open_id = None;
        let mut delete_id = None;
        let mut start_rename: Option<(i64, String)> = None;
        let mut finish_rename: Option<(i64, String)> = None;
        egui::ScrollArea::vertical()
            .id_salt("chat_conversations")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.conversations.is_empty() {
                    if self.search_query.trim().is_empty() {
                        ui.small("No conversations yet.");
                    } else {
                        ui.small("No conversations match that search.");
                    }
                }
                for summary in &rows {
                    let selected = self
                        .controller
                        .conversation
                        .as_ref()
                        .map(|conversation| conversation.id)
                        == Some(summary.id);
                    if self
                        .renaming
                        .as_ref()
                        .is_some_and(|(id, _)| *id == summary.id)
                    {
                        let buffer = self
                            .renaming
                            .as_mut()
                            .map(|(_, buffer)| buffer)
                            .expect("checked above");
                        ui.horizontal(|ui| {
                            let response = ui.add(
                                TextEdit::singleline(buffer)
                                    .desired_width(150.0)
                                    .char_limit(200),
                            );
                            let confirm = ui.small_button("✓");
                            confirm.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::accesskit::Role::Button,
                                    true,
                                    "Save new title",
                                )
                            });
                            if confirm.clicked() {
                                finish_rename = Some((summary.id, buffer.clone()));
                            }
                            let cancel = ui.small_button("×");
                            cancel.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::accesskit::Role::Button,
                                    true,
                                    "Cancel rename",
                                )
                            });
                            if cancel.clicked() {
                                finish_rename = Some((summary.id, String::new()));
                            }
                            if response.lost_focus()
                                && ui.input(|input| input.key_pressed(egui::Key::Enter))
                            {
                                finish_rename = Some((summary.id, buffer.clone()));
                            }
                        });
                    } else {
                        ui.horizontal(|ui| {
                            if ui
                                .selectable_label(selected, truncate(&summary.title, 26))
                                .on_hover_text(&summary.title)
                                .clicked()
                            {
                                open_id = Some(summary.id);
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let delete =
                                    ui.small_button("×").on_hover_text("Delete conversation");
                                delete.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::accesskit::Role::Button,
                                        true,
                                        format!("Delete {}", summary.title),
                                    )
                                });
                                if delete.clicked() {
                                    delete_id = Some(summary.id);
                                }
                                if summary.id > 0
                                    && (history_enabled
                                        || self
                                            .controller
                                            .conversation
                                            .as_ref()
                                            .is_some_and(|c| c.id == summary.id))
                                {
                                    let rename = ui.small_button("Rename").on_hover_text("Rename");
                                    rename.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::accesskit::Role::Button,
                                            true,
                                            format!("Rename {}", summary.title),
                                        )
                                    });
                                    if rename.clicked() {
                                        start_rename = Some((summary.id, summary.title.clone()));
                                    }
                                }
                            });
                        });
                        ui.small(format!(
                            "{} · {} · {}{}",
                            relative_time(summary.updated_at, now),
                            summary.model,
                            if summary.message_count == 1 {
                                "1 message".to_owned()
                            } else {
                                format!("{} messages", summary.message_count)
                            },
                            if summary.id < 0 { " · not saved" } else { "" }
                        ));
                    }
                    ui.add_space(4.0);
                }
            });

        if let Some((id, buffer)) = start_rename {
            if id > 0 && !history_enabled {
                self.controller.error = Some(
                    "History is off, so saved conversation titles cannot be changed.".to_owned(),
                );
            } else {
                self.renaming = Some((id, buffer));
            }
        }
        if let Some((id, buffer)) = finish_rename {
            self.renaming = None;
            if !buffer.trim().is_empty() {
                let result = if self
                    .controller
                    .conversation
                    .as_ref()
                    .is_some_and(|conversation| conversation.id == id)
                {
                    self.controller.rename(config, storage, &buffer)
                } else if id > 0 && history_enabled {
                    storage
                        .rename_conversation(id, &buffer)
                        .map_err(|error| error.to_string())
                } else {
                    Ok(())
                };
                match result {
                    Ok(()) => self.force_reload(storage),
                    Err(error) => self.controller.error = Some(error),
                }
            }
        }
        if let Some(id) = open_id {
            match self.controller.open(storage, id) {
                Ok(()) => self.sync_from_conversation(),
                Err(error) => self.controller.error = Some(error),
            }
        }
        if let Some(id) = delete_id {
            self.pending_delete = Some(id);
        }
        if let Some(id) = self.pending_delete {
            ui.separator();
            ui.label("Delete this conversation?");
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    let active = self
                        .controller
                        .conversation
                        .as_ref()
                        .map(|conversation| conversation.id)
                        == Some(id);
                    let result = if active {
                        let result = self.controller.delete_active(storage);
                        self.draft.clear();
                        result
                    } else {
                        storage
                            .delete_conversation(id)
                            .map_err(|error| error.to_string())
                    };
                    match result {
                        Ok(()) => self.force_reload(storage),
                        Err(error) => {
                            self.controller.error =
                                Some(format!("Could not delete conversation: {error}"));
                        }
                    }
                    self.pending_delete = None;
                }
                if ui.button("Cancel").clicked() {
                    self.pending_delete = None;
                }
            });
        }

        ui.separator();
        if self.confirm_clear {
            ui.label("Delete all saved conversations?");
            ui.horizontal(|ui| {
                if ui.button("Delete everything").clicked() {
                    match storage.clear_all_history() {
                        Ok(()) => {
                            self.on_history_cleared();
                            self.force_reload(storage);
                        }
                        Err(error) => {
                            self.controller.error =
                                Some(format!("Could not clear history: {error}"));
                        }
                    }
                    self.confirm_clear = false;
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_clear = false;
                }
            });
        } else if ui
            .button("Clear all history")
            .on_hover_text("Delete every saved conversation")
            .clicked()
        {
            self.confirm_clear = true;
        }

        if let Some(error) = &self.list_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if let Some(error) = &self.controller.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if config.history_enabled {
            ui.small("Usage is recorded with saved messages.");
        } else {
            ui.small("No conversation content or usage is written to disk.");
        }
        let _ = outcome;
    }

    fn sync_from_conversation(&mut self) {
        if let Some(conversation) = &self.controller.conversation {
            self.provider_id = Some(conversation.provider_id);
            self.model = conversation.model.clone();
        }
    }

    fn thread(
        &mut self,
        ui: &mut egui::Ui,
        config: &mut Config,
        storage: &Storage,
        outcome: &mut ChatOutcome,
    ) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.heading("Conversation");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.controller.is_streaming() && ui.button("Stop").clicked() {
                    self.controller.cancel(storage);
                    self.force_reload(storage);
                }
            });
        });

        if self.first_run_callout(config) {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Set up a provider").strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("Dismiss").clicked() {
                            outcome.dismiss_callout = true;
                        }
                    });
                });
                ui.small(
                    "No remote provider is configured. Add one in Providers and save a key before sending.",
                );
                if ui.button("Open Providers").clicked() {
                    outcome.open_providers = true;
                }
            });
            ui.add_space(6.0);
        }

        ui.horizontal_wrapped(|ui| {
            let provider_label = ui.label("Provider");
            let selected_name = self
                .provider_id
                .and_then(|id| config.provider(id))
                .map(|provider| provider.name.clone())
                .unwrap_or_else(|| "None".to_owned());
            let mut provider_changed = false;
            let provider_combo = egui::ComboBox::from_id_salt("chat_provider")
                .selected_text(selected_name)
                .width(160.0)
                .show_ui(ui, |ui| {
                    for provider in &config.providers {
                        if ui
                            .selectable_value(
                                &mut self.provider_id,
                                Some(provider.id),
                                &provider.name,
                            )
                            .changed()
                        {
                            provider_changed = true;
                        }
                    }
                });
            provider_combo.response.labelled_by(provider_label.id);
            if provider_changed {
                self.model = self
                    .provider_id
                    .and_then(|id| config.provider(id))
                    .map(default_model)
                    .unwrap_or_default();
            }
            ui.separator();
            let model_label = ui.label("Model");
            let model_field = ui.add(
                TextEdit::singleline(&mut self.model)
                    .desired_width(190.0)
                    .hint_text("model id"),
            );
            model_field.labelled_by(model_label.id);
            if let Some(provider) = self.provider_id.and_then(|id| config.provider(id))
                && !provider.models.is_empty()
            {
                ui.menu_button("Catalog", |ui| {
                    for model in &provider.models {
                        if ui.button(model.label()).clicked() {
                            self.model = model.id.clone();
                            ui.close();
                        }
                    }
                });
            }
        });

        ui.horizontal(|ui| {
            ui.checkbox(&mut self.show_system_prompt, "System prompt");
            if let Some(usage) = self.controller.conversation_usage(storage) {
                ui.separator();
                let currency = usage.currency.as_deref().unwrap_or("USD");
                ui.small(format!(
                    "{} in · {} out · ~{:.4} {currency}",
                    usage.input_tokens, usage.output_tokens, usage.estimated_cost
                ));
            }
        });
        if self.show_system_prompt {
            ui.add(
                TextEdit::multiline(&mut self.controller.system_prompt)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .hint_text("Optional system instructions for this conversation"),
            );
        }
        if let Some(notice) = &self.controller.notice {
            ui.small(notice);
        }
        if let Some(error) = &self.controller.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }

        ui.separator();
        let thread_height = (ui.available_height() - COMPOSER_RESERVE).max(MIN_THREAD_HEIGHT);
        egui::ScrollArea::vertical()
            .id_salt("chat_thread")
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .max_height(thread_height)
            .show(ui, |ui| {
                if self.controller.messages.is_empty() && self.controller.streaming.is_none() {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("Start a conversation").strong());
                        ui.small("Choose a provider and model above, then send a message.");
                    });
                }
                for message in &self.controller.messages {
                    show_message(ui, message);
                }
                if let Some(stream) = &self.controller.streaming {
                    show_streaming(ui, stream);
                }
            });

        ui.separator();
        ui.add(
            TextEdit::multiline(&mut self.draft)
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .hint_text("Message the model…"),
        );
        ui.horizontal(|ui| {
            let can_send = !self.controller.is_streaming() && !self.draft.trim().is_empty();
            let send_shortcut =
                ui.input(|input| input.modifiers.command && input.key_pressed(egui::Key::Enter));
            if ui
                .add_enabled(can_send, egui::Button::new("Send"))
                .on_hover_text(send_shortcut_label())
                .clicked()
                || (send_shortcut && can_send)
            {
                self.send(config, storage);
            }
            if self.controller.is_streaming() && ui.button("Stop").clicked() {
                self.controller.cancel(storage);
                self.force_reload(storage);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.small(send_shortcut_label());
            });
        });
    }

    fn first_run_callout(&self, config: &Config) -> bool {
        !config.first_run_callout_dismissed
            && !self.callout_dismissed
            && only_untouched_local_gateway(config)
    }

    fn send(&mut self, config: &Config, storage: &Storage) {
        let Some(provider_id) = self.provider_id else {
            self.controller.error = Some("Add a provider profile first.".to_owned());
            return;
        };
        let target = ChatTarget::new(provider_id, self.model.trim());
        match self
            .controller
            .send(config, storage, &target, &self.draft.clone())
        {
            Ok(()) => {
                self.draft.clear();
                self.force_reload(storage);
            }
            Err(error) => self.controller.error = Some(error),
        }
    }
}

/// True when the profile is exactly the default local gateway and nothing else
/// was configured. Detection is based on config state, not a profile id.
fn only_untouched_local_gateway(config: &Config) -> bool {
    if config.providers.len() != 1 {
        return false;
    }
    let provider = &config.providers[0];
    provider.protocol == Protocol::OpenAiChat
        && provider.auth == AuthMode::None
        && provider.default_model == "auto"
        && Url::parse(&provider.base_url)
            .ok()
            .is_some_and(|url| url.host_str() == Some("127.0.0.1"))
        && provider.models.iter().all(|model| model.id == "auto")
}

fn default_model(provider: &ProviderProfile) -> String {
    if !provider.default_model.trim().is_empty() {
        return provider.default_model.clone();
    }
    provider
        .models
        .first()
        .map(|model| model.id.clone())
        .unwrap_or_default()
}

fn send_shortcut_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Cmd+Enter to send"
    } else {
        "Ctrl+Enter to send"
    }
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut truncated: String = text.chars().take(limit.saturating_sub(1)).collect();
    truncated.push('…');
    truncated
}

/// Human-readable age from a stored UTC timestamp.
fn relative_time(timestamp: u64, now: u64) -> String {
    if timestamp == 0 || timestamp > now.saturating_add(60) {
        return "unknown".to_owned();
    }
    let seconds = now.saturating_sub(timestamp);
    match seconds {
        0..=59 => "just now".to_owned(),
        60..=3_599 => format!("{} min ago", seconds / 60),
        3_600..=86_399 => format!("{} h ago", seconds / 3_600),
        86_400..=172_799 => "yesterday".to_owned(),
        172_800..=2_591_999 => format!("{} days ago", seconds / 86_400),
        2_592_000..=31_535_999 => format!("{} months ago", seconds / 2_592_000),
        _ => format!("{} years ago", seconds / 31_536_000),
    }
}

fn show_message(ui: &mut egui::Ui, message: &StoredMessage) {
    let (label, color) = match message.role {
        MessageRole::User => ("You", ui.visuals().strong_text_color()),
        MessageRole::Assistant => ("Assistant", ui.visuals().text_color()),
        MessageRole::System => ("System", ui.visuals().weak_text_color()),
        MessageRole::Tool => (
            message.tool_name.as_deref().unwrap_or("Tool"),
            ui.visuals().weak_text_color(),
        ),
    };
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).strong().color(color));
        if message.status == MessageStatus::Cancelled {
            ui.small("(stopped)");
        }
        if message.status == MessageStatus::Failed {
            ui.colored_label(ui.visuals().error_fg_color, "(failed)");
        }
    });
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if message.content.trim().is_empty() {
            ui.weak("(no text)");
        } else {
            ui.add(egui::Label::new(&message.content).selectable(true).wrap());
        }
        for call in &message.tool_calls {
            ui.collapsing(format!("Tool call · {}", call.name), |ui| {
                let arguments = call.arguments_value().map_or_else(
                    |_| call.arguments.clone(),
                    |value| {
                        serde_json::to_string_pretty(&value)
                            .unwrap_or_else(|_| call.arguments.clone())
                    },
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(truncate(&arguments, MAX_TOOL_ARGUMENT_BYTES))
                            .monospace()
                            .small(),
                    )
                    .selectable(true)
                    .wrap(),
                );
            });
        }
        if let Some(usage) = &message.usage {
            ui.add_space(4.0);
            ui.small(format!(
                "Tokens: {} in · {} out · {} cached · {} total",
                usage.input_tokens,
                usage.output_tokens,
                usage.cached_input_tokens,
                usage.total_tokens()
            ));
        }
        if let Some(cost) = &message.cost {
            ui.small(format!("Estimated cost: {}", cost.label()));
        }
    });
    ui.add_space(6.0);
}

fn show_streaming(ui: &mut egui::Ui, stream: &crate::chat::controller::StreamingReply) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Assistant").strong());
        ui.spinner();
        ui.small(format!(
            "streaming · {}s",
            stream.started_at.elapsed().as_secs()
        ));
    });
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if !stream.reasoning.trim().is_empty() {
            ui.collapsing("Reasoning", |ui| {
                ui.add(
                    egui::Label::new(RichText::new(&stream.reasoning).small())
                        .selectable(true)
                        .wrap(),
                );
            });
        }
        if stream.text.is_empty() {
            ui.weak("Waiting for the provider…");
        } else {
            ui.add(
                egui::Label::new(format!("{} |", stream.text))
                    .selectable(true)
                    .wrap(),
            );
        }
    });
    ui.add_space(6.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ModelConfig, ProviderProfile};

    #[test]
    fn chat_renders_with_no_conversations() {
        let storage = Storage::open_in_memory().unwrap();
        let mut config = Config::default();
        let mut screen = ChatScreen::new(&config, &storage);
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1_080.0, 720.0),
            )),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            screen.show(ui, &mut config, &storage);
        });
        output.textures_delta.clear();
        assert_eq!(storage.conversation_count().unwrap(), 0);
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("héllo wörld", 6), "héllo…");
    }

    #[test]
    fn default_model_falls_back_from_profile_default_to_catalog() {
        let mut provider = ProviderProfile::new(
            1,
            "p",
            Protocol::OpenAiChat,
            "http://127.0.0.1:1/v1",
            AuthMode::None,
            "",
        );
        assert_eq!(default_model(&provider), "");
        provider.models = vec![ModelConfig::new("m")];
        assert_eq!(default_model(&provider), "m");
        provider.default_model = "custom".to_owned();
        assert_eq!(default_model(&provider), "custom");
    }

    #[test]
    fn relative_time_covers_the_documented_ranges() {
        let now = 1_700_000_000;
        assert_eq!(relative_time(now, now), "just now");
        assert_eq!(relative_time(now - 120, now), "2 min ago");
        assert_eq!(relative_time(now - 7_200, now), "2 h ago");
        assert_eq!(relative_time(now - 90_000, now), "yesterday");
        assert_eq!(relative_time(now - 4 * 86_400, now), "4 days ago");
        assert_eq!(relative_time(now - 65 * 86_400, now), "2 months ago");
        assert_eq!(relative_time(now - 400 * 86_400, now), "1 years ago");
        assert_eq!(relative_time(now + 600, now), "unknown");
        assert_eq!(relative_time(0, now), "unknown");
    }

    #[test]
    fn first_run_detection_uses_config_state_not_an_id() {
        let mut config = Config::default();
        assert!(only_untouched_local_gateway(&config));
        let mut renamed = config.providers[0].clone();
        renamed.id = 99;
        config.providers = vec![renamed];
        assert!(only_untouched_local_gateway(&config));
        config.providers[0].name = "My gateway".to_owned();
        assert!(only_untouched_local_gateway(&config));
        config.providers[0].base_url = "https://api.example.com/v1".to_owned();
        assert!(!only_untouched_local_gateway(&config));
        config.providers[0].base_url = "http://127.0.0.1:3001/v1".to_owned();
        config.providers.push(ProviderProfile::new(
            2,
            "Other",
            Protocol::OpenAiChat,
            "https://api.example.com/v1",
            AuthMode::Bearer,
            "m",
        ));
        assert!(!only_untouched_local_gateway(&config));
    }
}
