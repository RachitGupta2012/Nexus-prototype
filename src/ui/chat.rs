//! Conversation screen: history sidebar, message thread, and composer.

use std::time::Duration;

use eframe::egui::{self, Align, Layout, RichText, TextEdit};

use crate::chat::controller::{ChatController, ChatTarget};
use crate::chat::{ConversationSummary, MessageRole, MessageStatus, StoredMessage};
use crate::config::{Config, ProviderProfile};
use crate::storage::Storage;

const SIDEBAR_WIDTH: f32 = 240.0;
const MIN_THREAD_HEIGHT: f32 = 160.0;
const COMPOSER_RESERVE: f32 = 132.0;
const MAX_CONVERSATIONS: u32 = 200;

pub struct ChatScreen {
    controller: ChatController,
    draft: String,
    provider_id: Option<u64>,
    model: String,
    conversations: Vec<ConversationSummary>,
    show_system_prompt: bool,
    pending_delete: Option<i64>,
    list_error: Option<String>,
}

impl ChatScreen {
    pub fn new(config: &Config, storage: &Storage) -> Self {
        let mut screen = Self {
            controller: ChatController::new(),
            draft: String::new(),
            provider_id: None,
            model: String::new(),
            conversations: Vec::new(),
            show_system_prompt: false,
            pending_delete: None,
            list_error: None,
        };
        screen.reset_target(config);
        screen.reload(storage);
        screen
    }

    pub fn show(&mut self, ui: &mut egui::Ui, config: &Config, storage: &Storage) {
        self.sync_target(config);
        if self.controller.is_streaming() {
            self.controller.poll(config, storage);
            ui.ctx().request_repaint_after(Duration::from_millis(40));
            if !self.controller.is_streaming() {
                self.reload(storage);
            }
        }

        let height = ui.available_height();
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(SIDEBAR_WIDTH, height),
                Layout::top_down(Align::LEFT),
                |ui| self.sidebar(ui, config, storage),
            );
            ui.separator();
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width().max(320.0), height),
                Layout::top_down(Align::LEFT),
                |ui| self.thread(ui, config, storage),
            );
        });
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

    fn reload(&mut self, storage: &Storage) {
        match storage.list_conversations(MAX_CONVERSATIONS) {
            Ok(conversations) => {
                self.conversations = conversations;
                self.list_error = None;
            }
            Err(error) => self.list_error = Some(format!("Could not load conversations: {error}")),
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui, config: &Config, storage: &Storage) {
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
                    self.reset_target(config);
                }
            });
        });
        ui.separator();

        let mut open_id = None;
        let mut delete_id = None;
        egui::ScrollArea::vertical()
            .id_salt("chat_conversations")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.conversations.is_empty() {
                    ui.small("No conversations yet.");
                }
                for summary in &self.conversations {
                    let selected = self
                        .controller
                        .conversation
                        .as_ref()
                        .map(|conversation| conversation.id)
                        == Some(summary.id);
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(selected, truncate(&summary.title, 30))
                            .on_hover_text(&summary.title)
                            .clicked()
                        {
                            open_id = Some(summary.id);
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .small_button("×")
                                .on_hover_text("Delete conversation")
                                .clicked()
                            {
                                delete_id = Some(summary.id);
                            }
                        });
                    });
                    ui.small(format!(
                        "{} · {} msg{}",
                        summary.model,
                        summary.message_count,
                        if summary.message_count == 1 { "" } else { "s" }
                    ));
                    ui.add_space(4.0);
                }
            });

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
                    match storage.delete_conversation(id) {
                        Ok(()) => {
                            if active {
                                self.controller.start_new();
                                self.draft.clear();
                                self.reset_target(config);
                            }
                            self.reload(storage);
                        }
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
        if let Some(error) = &self.list_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if let Some(error) = &self.controller.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }

    fn sync_from_conversation(&mut self) {
        if let Some(conversation) = &self.controller.conversation {
            self.provider_id = Some(conversation.provider_id);
            self.model = conversation.model.clone();
        }
    }

    fn thread(&mut self, ui: &mut egui::Ui, config: &Config, storage: &Storage) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.heading("Conversation");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.controller.is_streaming() && ui.button("Stop").clicked() {
                    self.controller.cancel(storage);
                    self.reload(storage);
                }
            });
        });

        ui.horizontal_wrapped(|ui| {
            ui.label("Provider");
            let selected_name = self
                .provider_id
                .and_then(|id| config.provider(id))
                .map(|provider| provider.name.clone())
                .unwrap_or_else(|| "None".to_owned());
            let mut provider_changed = false;
            egui::ComboBox::from_id_salt("chat_provider")
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
            if provider_changed {
                self.model = self
                    .provider_id
                    .and_then(|id| config.provider(id))
                    .map(default_model)
                    .unwrap_or_default();
            }
            ui.separator();
            ui.label("Model");
            ui.add(
                TextEdit::singleline(&mut self.model)
                    .desired_width(190.0)
                    .hint_text("model id"),
            );
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
            if let Some((input, output, cost, currency)) =
                self.controller.conversation_usage(storage)
            {
                ui.separator();
                ui.small(format!("{input} in · {output} out · ~{cost:.4} {currency}"));
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
                self.reload(storage);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.small(send_shortcut_label());
            });
        });
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
                self.reload(storage);
            }
            Err(error) => self.controller.error = Some(error),
        }
    }
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
                ui.small(format!("id: {}", call.id));
                ui.add(
                    egui::Label::new(RichText::new(&call.arguments).monospace().small())
                        .selectable(true)
                        .wrap(),
                );
            });
        }
        if let Some(usage) = &message.usage {
            ui.add_space(4.0);
            ui.small(format!(
                "Tokens: {} in · {} out · {} cached",
                usage.input_tokens, usage.output_tokens, usage.cached_input_tokens
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
        ui.small("streaming…");
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
    use crate::config::{AuthMode, ModelConfig, Protocol};

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
}
