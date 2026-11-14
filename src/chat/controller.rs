//! Conversation state machine shared by the chat screen.
//!
//! The controller owns the active conversation and turns provider stream events
//! into persisted messages. It deliberately contains no egui types so it can be
//! tested without a window.

use std::time::Instant;

use crate::chat::{
    Conversation, ConversationId, CostEstimate, MAX_SYSTEM_PROMPT_BYTES, MAX_USER_MESSAGE_BYTES,
    MessageRole, MessageStatus, StoredMessage, TokenUsage, ToolCall,
};
use crate::config::Config;
use crate::credentials;
use crate::provider::{
    ChatEvent, ChatRequest, ProviderController, ProviderError, ProviderSettings,
};
use crate::storage::{NewMessage, Storage, StorageError, unix_time_now};

const MAX_TITLE_CHARS: usize = 72;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatTarget {
    pub provider_id: u64,
    pub model: String,
}

impl ChatTarget {
    pub fn new(provider_id: u64, model: impl Into<String>) -> Self {
        Self {
            provider_id,
            model: model.into(),
        }
    }
}

pub struct StreamingReply {
    controller: ProviderController,
    pub text: String,
    pub reasoning: String,
    pub started_at: Instant,
    target: ChatTarget,
}

pub struct ChatController {
    pub conversation: Option<Conversation>,
    pub messages: Vec<StoredMessage>,
    pub streaming: Option<StreamingReply>,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub system_prompt: String,
}

impl Default for ChatController {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatController {
    pub fn new() -> Self {
        Self {
            conversation: None,
            messages: Vec::new(),
            streaming: None,
            error: None,
            notice: None,
            system_prompt: String::new(),
        }
    }

    pub fn is_streaming(&self) -> bool {
        self.streaming.is_some()
    }

    pub fn start_new(&mut self) {
        if self.is_streaming() {
            return;
        }
        self.conversation = None;
        self.messages.clear();
        self.system_prompt.clear();
        self.error = None;
        self.notice = None;
    }

    pub fn open(&mut self, storage: &Storage, id: ConversationId) -> Result<(), String> {
        if self.is_streaming() {
            return Err("Stop the current response before switching conversations.".to_owned());
        }
        let conversation = storage
            .load_conversation(id)
            .map_err(storage_message)?
            .ok_or_else(|| "That conversation no longer exists.".to_owned())?;
        let messages = storage.load_messages(id, 1_000).map_err(storage_message)?;
        self.system_prompt = conversation.system_prompt.clone();
        self.conversation = Some(conversation);
        self.messages = messages;
        self.error = None;
        self.notice = None;
        Ok(())
    }

    pub fn delete_active(&mut self, storage: &Storage) -> Result<(), String> {
        if let Some(conversation) = &self.conversation {
            storage
                .delete_conversation(conversation.id)
                .map_err(storage_message)?;
        }
        self.start_new();
        Ok(())
    }

    pub fn send(
        &mut self,
        config: &Config,
        storage: &Storage,
        target: &ChatTarget,
        text: &str,
    ) -> Result<(), String> {
        if self.is_streaming() {
            return Err("A response is already streaming.".to_owned());
        }
        let text = text.trim();
        if text.is_empty() {
            return Err("Write a message before sending.".to_owned());
        }
        if text.len() > MAX_USER_MESSAGE_BYTES {
            return Err(format!(
                "The message is too long. Keep it under {} KB.",
                MAX_USER_MESSAGE_BYTES / 1_024
            ));
        }
        if self.system_prompt.len() > MAX_SYSTEM_PROMPT_BYTES {
            return Err(format!(
                "The system prompt is too long. Keep it under {} KB.",
                MAX_SYSTEM_PROMPT_BYTES / 1_024
            ));
        }
        let provider = config
            .provider(target.provider_id)
            .ok_or_else(|| "Select a provider profile first.".to_owned())?;
        let model = target.model.trim();
        if model.is_empty() {
            return Err("Select a model first.".to_owned());
        }
        let settings = ProviderSettings {
            profile_id: provider.id,
            protocol: provider.protocol,
            base_url: provider.base_url.clone(),
            auth: provider.auth,
            model: model.to_owned(),
        };
        settings.validate().map_err(provider_message)?;
        let api_key = if settings.requires_key() {
            let account = settings.account().map_err(provider_message)?;
            credentials::load(&account)
                .map_err(|error| format!("Could not read the saved API key: {error}"))?
                .ok_or_else(|| {
                    "No API key is saved for this provider. Add one in Providers.".to_owned()
                })?
                .into()
        } else {
            None
        };

        let conversation_id = match &self.conversation {
            Some(conversation) => {
                let changed = conversation.provider_id != provider.id
                    || conversation.protocol != provider.protocol
                    || conversation.model != model
                    || conversation.system_prompt != self.system_prompt;
                let conversation_id = conversation.id;
                if changed {
                    storage
                        .update_conversation_model(
                            conversation_id,
                            provider.id,
                            provider.protocol,
                            model,
                        )
                        .map_err(storage_message)?;
                    storage
                        .update_system_prompt(conversation_id, &self.system_prompt)
                        .map_err(storage_message)?;
                    let conversation = self.conversation.as_mut().expect("checked above");
                    conversation.provider_id = provider.id;
                    conversation.protocol = provider.protocol;
                    conversation.model = model.to_owned();
                    conversation.system_prompt = self.system_prompt.clone();
                    conversation.updated_at = unix_time_now();
                }
                conversation_id
            }
            None => {
                let title = title_from(text);
                let id = storage
                    .create_conversation(
                        provider.id,
                        provider.protocol,
                        model,
                        &title,
                        &self.system_prompt,
                    )
                    .map_err(storage_message)?;
                self.conversation = storage.load_conversation(id).map_err(storage_message)?;
                id
            }
        };

        let now = unix_time_now();
        let user_id = storage
            .append_message(
                conversation_id,
                NewMessage {
                    role: MessageRole::User,
                    content: text,
                    tool_calls: &[],
                    tool_call_id: None,
                    tool_name: None,
                    status: MessageStatus::Complete,
                    usage: None,
                    cost: None,
                    provider_id: Some(provider.id),
                    protocol: Some(provider.protocol),
                    model: Some(model),
                },
            )
            .map_err(storage_message)?;
        self.messages.push(StoredMessage {
            id: user_id,
            role: MessageRole::User,
            content: text.to_owned(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            tool_name: None,
            status: MessageStatus::Complete,
            created_at: now,
            usage: None,
            cost: None,
        });

        let request = ChatRequest {
            system_prompt: self.system_prompt.clone(),
            messages: self.history_for_request(),
            tools: Vec::new(),
            temperature: None,
            max_output_tokens: None,
        };
        let controller =
            ProviderController::start(settings, api_key, request).map_err(provider_message)?;
        self.streaming = Some(StreamingReply {
            controller,
            text: String::new(),
            reasoning: String::new(),
            started_at: Instant::now(),
            target: target.clone(),
        });
        self.error = None;
        self.notice = None;
        Ok(())
    }

    pub fn poll(&mut self, config: &Config, storage: &Storage) {
        let mut finished: Option<(Vec<ToolCall>, Option<TokenUsage>)> = None;
        let mut failed: Option<ProviderError> = None;
        if let Some(stream) = self.streaming.as_mut() {
            for event in stream.controller.poll() {
                match event {
                    ChatEvent::TextDelta(delta) => stream.text.push_str(&delta),
                    ChatEvent::ReasoningDelta(delta) => stream.reasoning.push_str(&delta),
                    ChatEvent::Finished { tool_calls, usage } => {
                        finished = Some((tool_calls, usage));
                        break;
                    }
                    ChatEvent::Failed(error) => {
                        failed = Some(error);
                        break;
                    }
                }
            }
        }
        if let Some((tool_calls, usage)) = finished {
            self.finish(config, storage, tool_calls, usage);
        } else if let Some(error) = failed {
            self.fail(storage, &error.to_string());
        }
    }

    pub fn cancel(&mut self, storage: &Storage) {
        let Some(stream) = self.streaming.take() else {
            return;
        };
        stream.controller.cancel();
        let content = stream.text.trim().to_owned();
        if !content.is_empty() {
            let _ = self.persist_assistant(
                storage,
                stream.target,
                content,
                Vec::new(),
                None,
                None,
                MessageStatus::Cancelled,
            );
        }
        self.notice = Some("Response stopped.".to_owned());
    }

    fn finish(
        &mut self,
        config: &Config,
        storage: &Storage,
        tool_calls: Vec<ToolCall>,
        usage: Option<TokenUsage>,
    ) {
        let Some(stream) = self.streaming.take() else {
            return;
        };
        let content = stream.text.trim_end().to_owned();
        if content.is_empty() && tool_calls.is_empty() {
            self.notice = Some("The provider returned an empty response.".to_owned());
            return;
        }
        let cost = usage
            .as_ref()
            .and_then(|usage| self.cost_for(config, &stream.target, usage));
        match self.persist_assistant(
            storage,
            stream.target,
            content,
            tool_calls.clone(),
            usage,
            cost.clone(),
            MessageStatus::Complete,
        ) {
            Ok(Some(message)) => {
                self.messages.push(message);
                if !tool_calls.is_empty() {
                    self.notice = Some(format!(
                        "The model requested {} tool call(s). MCP tool execution lands in the MCP phase.",
                        tool_calls.len()
                    ));
                }
                if let Some(cost) = cost {
                    self.notice = Some(format!("Estimated cost {}", cost.label()));
                }
            }
            Ok(None) => {}
            Err(error) => self.error = Some(error),
        }
    }

    fn fail(&mut self, storage: &Storage, message: &str) {
        let Some(stream) = self.streaming.take() else {
            return;
        };
        let content = stream.text.trim_end().to_owned();
        if !content.is_empty() {
            let _ = self.persist_assistant(
                storage,
                stream.target,
                content,
                Vec::new(),
                None,
                None,
                MessageStatus::Failed,
            );
        }
        self.error = Some(message.to_owned());
    }

    fn persist_assistant(
        &mut self,
        storage: &Storage,
        target: ChatTarget,
        content: String,
        tool_calls: Vec<ToolCall>,
        usage: Option<TokenUsage>,
        cost: Option<CostEstimate>,
        status: MessageStatus,
    ) -> Result<Option<StoredMessage>, String> {
        let Some(conversation_id) = self.conversation.as_ref().map(|c| c.id) else {
            return Ok(None);
        };
        let protocol = self.conversation.as_ref().map(|c| c.protocol);
        let model = self.conversation.as_ref().map(|c| c.model.clone());
        let now = unix_time_now();
        let id = storage
            .append_message(
                conversation_id,
                NewMessage {
                    role: MessageRole::Assistant,
                    content: &content,
                    tool_calls: &tool_calls,
                    tool_call_id: None,
                    tool_name: None,
                    status,
                    usage: usage.as_ref(),
                    cost: cost.as_ref(),
                    provider_id: Some(target.provider_id),
                    protocol,
                    model: model.as_deref(),
                },
            )
            .map_err(storage_message)?;
        Ok(Some(StoredMessage {
            id,
            role: MessageRole::Assistant,
            content,
            tool_calls,
            tool_call_id: None,
            tool_name: None,
            status,
            created_at: now,
            usage,
            cost,
        }))
    }

    fn cost_for(
        &self,
        config: &Config,
        target: &ChatTarget,
        usage: &TokenUsage,
    ) -> Option<CostEstimate> {
        let provider = config.provider(target.provider_id)?;
        let model = provider.model(&target.model)?;
        let pricing = model.pricing.as_ref()?;
        CostEstimate::from_usage(pricing, usage)
    }

    /// Provider history for the next request.
    ///
    /// Tool turns are excluded until the MCP client can replay their results;
    /// dangling tool-call references are rejected by stricter gateways.
    fn history_for_request(&self) -> Vec<StoredMessage> {
        self.messages
            .iter()
            .filter(|message| match message.role {
                MessageRole::User => true,
                MessageRole::Assistant => {
                    !message.content.trim().is_empty()
                        && message.tool_calls.is_empty()
                        && matches!(
                            message.status,
                            MessageStatus::Complete | MessageStatus::Cancelled
                        )
                }
                MessageRole::System | MessageRole::Tool => false,
            })
            .cloned()
            .collect()
    }

    pub fn conversation_usage(&self, storage: &Storage) -> Option<(u64, u64, f64, String)> {
        let conversation = self.conversation.as_ref()?;
        let (input, output, currency, cost) =
            storage.conversation_usage(conversation.id).ok().flatten()?;
        (input > 0 || output > 0).then(|| {
            (
                input,
                output,
                cost,
                currency.unwrap_or_else(|| "USD".to_owned()),
            )
        })
    }
}

fn title_from(text: &str) -> String {
    let first_line = text.lines().next().unwrap_or(text).trim();
    if first_line.is_empty() {
        return "New conversation".to_owned();
    }
    let mut title: String = first_line.chars().take(MAX_TITLE_CHARS).collect();
    if first_line.chars().count() > MAX_TITLE_CHARS {
        title.push('…');
    }
    title
}

fn storage_message(error: StorageError) -> String {
    error.to_string()
}

fn provider_message(error: ProviderError) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthMode, ModelConfig, ModelPricing, Protocol, ProviderProfile};

    fn config_with_pricing() -> Config {
        let mut provider = ProviderProfile::new(
            1,
            "Test",
            Protocol::OpenAiChat,
            "http://127.0.0.1:3001/v1",
            AuthMode::None,
            "test-model",
        );
        provider.models = vec![ModelConfig {
            id: "test-model".into(),
            display_name: "Test model".into(),
            context_window: Some(8_192),
            supports_tools: Some(true),
            pricing: Some(ModelPricing {
                input_per_mtok: 1.0,
                output_per_mtok: 2.0,
                cached_input_per_mtok: None,
                currency: "USD".into(),
                reviewed_at_unix_secs: None,
            }),
        }];
        Config {
            providers: vec![provider],
            default_provider_id: Some(1),
            ..Config::default()
        }
    }

    #[test]
    fn title_uses_the_first_line_and_truncates() {
        assert_eq!(title_from("Hello\nthere"), "Hello");
        let long = "word ".repeat(40);
        let title = title_from(&long);
        assert!(title.ends_with('…'));
        assert_eq!(title.chars().count(), MAX_TITLE_CHARS + 1);
    }

    #[test]
    fn cost_requires_matching_pricing_and_usage() {
        let controller = ChatController::new();
        let config = config_with_pricing();
        let target = ChatTarget::new(1, "test-model");
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            cached_input_tokens: 0,
        };
        let cost = controller.cost_for(&config, &target, &usage).unwrap();
        assert!((cost.amount - 2.0).abs() < 1e-9);
        let missing_model = ChatTarget::new(1, "other");
        assert!(
            controller
                .cost_for(&config, &missing_model, &usage)
                .is_none()
        );
    }
}
