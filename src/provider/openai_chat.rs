use std::collections::BTreeMap;
use std::sync::mpsc::SyncSender;

use serde_json::{Value, json};
use tokio::sync::watch;

use super::http::{self, SseEvent};
use super::{
    ChatEvent, ChatRequest, FinishReason, MAX_TOOL_CALLS, MAX_TOOL_DEFINITION_BYTES,
    MAX_TOOL_DESCRIPTION_BYTES, ProviderError, ProviderSettings, send_event,
};
use crate::chat::{
    MAX_ASSISTANT_MESSAGE_BYTES, MAX_TOOL_ARGUMENT_BYTES, MessageRole, StoredMessage, TokenUsage,
    ToolCall,
};

pub async fn stream(
    settings: &ProviderSettings,
    api_key: Option<&str>,
    request: &ChatRequest,
    sender: &SyncSender<ChatEvent>,
    mut cancel: watch::Receiver<bool>,
) -> Result<(), ProviderError> {
    let body = build_body(settings, request)?;
    let mut state = StreamState::new(sender);
    http::post_sse(settings, api_key, &body, &mut cancel, |event| {
        state.handle(event)
    })
    .await?;
    if !state.finished {
        state.emit_finished()?;
    }
    Ok(())
}

fn build_body(settings: &ProviderSettings, request: &ChatRequest) -> Result<Value, ProviderError> {
    let mut messages = Vec::new();
    if !request.system_prompt.trim().is_empty() {
        messages.push(json!({
            "role": "system",
            "content": request.system_prompt,
        }));
    }
    for message in &request.messages {
        push_message(&mut messages, message);
    }
    let mut body = json!({
        "model": settings.model.trim(),
        "messages": messages,
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    if let Some(temperature) = request.temperature {
        if !temperature.is_finite() || !(0.0..=2.0).contains(&temperature) {
            return Err(ProviderError::InvalidConfig(
                "temperature must be between 0 and 2",
            ));
        }
        body["temperature"] = json!(temperature);
    }
    if let Some(max_output_tokens) = request.max_output_tokens {
        if max_output_tokens == 0 {
            return Err(ProviderError::InvalidConfig(
                "maximum output tokens must be greater than zero",
            ));
        }
        body["max_tokens"] = json!(max_output_tokens);
    }
    if !request.tools.is_empty() {
        if request.tools.len() > MAX_TOOL_CALLS {
            return Err(ProviderError::InvalidConfig("too many tool definitions"));
        }
        let mut tools = Vec::new();
        for tool in &request.tools {
            if tool.name.is_empty() || tool.name.len() > 64 {
                return Err(ProviderError::InvalidConfig("tool name is invalid"));
            }
            if !tool.parameters.is_object() {
                return Err(ProviderError::InvalidConfig(
                    "tool parameters must be a JSON schema object",
                ));
            }
            let description =
                &tool.description[..tool.description.len().min(MAX_TOOL_DESCRIPTION_BYTES)];
            let serialized = serde_json::to_string(&tool.parameters)
                .map_err(|_| ProviderError::InvalidResponse)?;
            if serialized.len() > MAX_TOOL_DEFINITION_BYTES {
                return Err(ProviderError::InvalidConfig("tool definition is too large"));
            }
            tools.push(json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": description,
                    "parameters": tool.parameters,
                }
            }));
        }
        body["tools"] = Value::Array(tools);
        body["tool_choice"] = json!("auto");
    }
    Ok(body)
}

fn push_message(messages: &mut Vec<Value>, message: &StoredMessage) {
    match message.role {
        MessageRole::System | MessageRole::User => {
            messages.push(json!({
                "role": message.role.as_str(),
                "content": message.content,
            }));
        }
        MessageRole::Assistant => {
            if message.tool_calls.is_empty() {
                messages.push(json!({
                    "role": "assistant",
                    "content": message.content,
                }));
            } else {
                let tool_calls: Vec<Value> = message
                    .tool_calls
                    .iter()
                    .map(|call| {
                        json!({
                            "id": call.id,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                "arguments": call.arguments,
                            }
                        })
                    })
                    .collect();
                messages.push(json!({
                    "role": "assistant",
                    "content": if message.content.is_empty() { Value::Null } else { json!(message.content) },
                    "tool_calls": tool_calls,
                }));
            }
        }
        MessageRole::Tool => {
            if let Some(call_id) = &message.tool_call_id {
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": message.content,
                }));
            }
        }
    }
}

struct StreamState<'a> {
    sender: &'a SyncSender<ChatEvent>,
    text_bytes: usize,
    tool_calls: BTreeMap<u64, ToolCallBuilder>,
    usage: Option<TokenUsage>,
    finished: bool,
}

struct ToolCallBuilder {
    id: String,
    name: String,
    arguments: String,
}

impl<'a> StreamState<'a> {
    fn new(sender: &'a SyncSender<ChatEvent>) -> Self {
        Self {
            sender,
            text_bytes: 0,
            tool_calls: BTreeMap::new(),
            usage: None,
            finished: false,
        }
    }

    fn handle(&mut self, event: SseEvent) -> Result<(), ProviderError> {
        if event.event.as_deref() == Some("error") {
            return Err(ProviderError::RequestFailed);
        }
        let data = event.data.trim();
        if data.is_empty() {
            return Ok(());
        }
        if data == "[DONE]" {
            return self.emit_finished();
        }
        let value: Value =
            serde_json::from_str(data).map_err(|_| ProviderError::InvalidResponse)?;
        if value.get("error").is_some() {
            return Err(ProviderError::RequestFailed);
        }
        if let Some(usage) = value.get("usage").and_then(parse_usage) {
            self.usage = Some(usage);
        }
        let Some(choice) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return Ok(());
        };
        let Some(delta) = choice.get("delta") else {
            return Ok(());
        };
        if let Some(content) = delta.get("content").and_then(text_content) {
            self.emit_text(content)?;
        }
        for key in ["reasoning_content", "reasoning"] {
            if let Some(reasoning) = delta.get(key).and_then(text_content) {
                self.emit_reasoning(reasoning)?;
            }
        }
        if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for tool_call in tool_calls {
                self.push_tool_call(tool_call)?;
            }
        }
        Ok(())
    }

    fn emit_text(&mut self, text: String) -> Result<(), ProviderError> {
        if text.is_empty() {
            return Ok(());
        }
        self.text_bytes = self.text_bytes.saturating_add(text.len());
        if self.text_bytes > MAX_ASSISTANT_MESSAGE_BYTES {
            return Err(ProviderError::ResponseTooLarge);
        }
        send_event(self.sender, ChatEvent::TextDelta(text))
    }

    fn emit_reasoning(&mut self, text: String) -> Result<(), ProviderError> {
        if text.is_empty() {
            return Ok(());
        }
        self.text_bytes = self.text_bytes.saturating_add(text.len());
        if self.text_bytes > MAX_ASSISTANT_MESSAGE_BYTES {
            return Err(ProviderError::ResponseTooLarge);
        }
        send_event(self.sender, ChatEvent::ReasoningDelta(text))
    }

    fn push_tool_call(&mut self, tool_call: &Value) -> Result<(), ProviderError> {
        let index = tool_call.get("index").and_then(Value::as_u64).unwrap_or(0);
        if !self.tool_calls.contains_key(&index) && self.tool_calls.len() >= MAX_TOOL_CALLS {
            return Err(ProviderError::InvalidResponse);
        }
        let builder = self
            .tool_calls
            .entry(index)
            .or_insert_with(|| ToolCallBuilder {
                id: String::new(),
                name: String::new(),
                arguments: String::new(),
            });
        if let Some(id) = tool_call.get("id").and_then(Value::as_str)
            && !id.is_empty()
        {
            builder.id = id.chars().take(128).collect();
        }
        if let Some(function) = tool_call.get("function") {
            if let Some(name) = function.get("name").and_then(Value::as_str) {
                if builder.name.len().saturating_add(name.len()) > 128 {
                    return Err(ProviderError::ResponseTooLarge);
                }
                builder.name.push_str(name);
            }
            if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                if builder.arguments.len().saturating_add(arguments.len()) > MAX_TOOL_ARGUMENT_BYTES
                {
                    return Err(ProviderError::ResponseTooLarge);
                }
                builder.arguments.push_str(arguments);
            }
        }
        Ok(())
    }

    fn emit_finished(&mut self) -> Result<(), ProviderError> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        let mut tool_calls = Vec::new();
        for (index, builder) in std::mem::take(&mut self.tool_calls) {
            if builder.name.is_empty() {
                continue;
            }
            tool_calls.push(ToolCall {
                id: if builder.id.is_empty() {
                    format!("call_{index}")
                } else {
                    builder.id
                },
                name: builder.name,
                arguments: if builder.arguments.trim().is_empty() {
                    "{}".to_owned()
                } else {
                    builder.arguments
                },
            });
        }
        if tool_calls.len() > MAX_TOOL_CALLS {
            return Err(ProviderError::ResponseTooLarge);
        }
        send_event(
            self.sender,
            ChatEvent::Finished {
                tool_calls,
                usage: self.usage.clone(),
                finish: FinishReason::Completed,
            },
        )
    }
}

fn text_content(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_owned());
    }
    let parts = value.as_array()?;
    let mut text = String::new();
    for part in parts {
        if let Some(value) = part.get("text").and_then(Value::as_str) {
            text.push_str(value);
        }
    }
    if text.is_empty() { None } else { Some(text) }
}

fn parse_usage(value: &Value) -> Option<TokenUsage> {
    let input = value
        .get("prompt_tokens")
        .or_else(|| value.get("input_tokens"))
        .and_then(Value::as_u64)?;
    let output = value
        .get("completion_tokens")
        .or_else(|| value.get("output_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cached = value
        .pointer("/prompt_tokens_details/cached_tokens")
        .or_else(|| value.pointer("/input_tokens_details/cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if input == 0 && output == 0 && cached == 0 {
        return None;
    }
    Some(TokenUsage {
        input_tokens: input,
        output_tokens: output,
        cached_input_tokens: cached,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{MessageStatus, ToolCall};
    use crate::config::{AuthMode, Protocol};
    use crate::provider::ChatEvent;
    use std::sync::mpsc;

    fn settings() -> ProviderSettings {
        ProviderSettings {
            profile_id: 1,
            protocol: Protocol::OpenAiChat,
            base_url: "https://api.example.test/v1".into(),
            auth: AuthMode::Bearer,
            model: "test-model".into(),
        }
    }

    fn message(role: MessageRole, content: &str) -> StoredMessage {
        StoredMessage {
            id: 1,
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            tool_name: None,
            status: MessageStatus::Complete,
            created_at: 0,
            usage: None,
            cost: None,
        }
    }

    #[test]
    fn request_body_maps_roles_and_tools() {
        let mut assistant = message(MessageRole::Assistant, "checking");
        assistant.tool_calls = vec![ToolCall {
            id: "call_1".into(),
            name: "files__read".into(),
            arguments: "{\"path\":\"/tmp/a\"}".into(),
        }];
        let mut tool = message(MessageRole::Tool, "contents");
        tool.tool_call_id = Some("call_1".into());
        tool.tool_name = Some("files__read".into());
        let request = ChatRequest {
            system_prompt: "Be concise".into(),
            messages: vec![message(MessageRole::User, "hello"), assistant, tool],
            tools: vec![super::super::ToolDefinition {
                name: "files__read".into(),
                description: "Read a file".into(),
                parameters: json!({"type": "object", "properties": {}}),
            }],
            temperature: Some(0.2),
            max_output_tokens: Some(100),
        };
        let body = build_body(&settings(), &request).unwrap();
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["stream"], true);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(
            body["messages"][2]["tool_calls"][0]["function"]["name"],
            "files__read"
        );
        assert_eq!(body["messages"][3]["role"], "tool");
        assert_eq!(body["tools"][0]["function"]["name"], "files__read");
        assert_eq!(body["tool_choice"], "auto");
    }

    #[test]
    fn stream_state_collects_text_usage_and_tool_calls() {
        let (sender, receiver) = mpsc::sync_channel(16);
        let mut state = StreamState::new(&sender);
        state
            .handle(SseEvent {
                event: None,
                data: json!({
                    "choices": [{"delta": {"content": "Hello "}}]
                })
                .to_string(),
            })
            .unwrap();
        state
            .handle(SseEvent {
                event: None,
                data: json!({
                    "choices": [{"delta": {"tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": {"name": "files__", "arguments": "{\"path\":"}
                    }]}}]
                })
                .to_string(),
            })
            .unwrap();
        state
            .handle(SseEvent {
                event: None,
                data: json!({
                    "choices": [{"delta": {"tool_calls": [{
                        "index": 0,
                        "function": {"name": "read", "arguments": "\"/tmp/a\"}"}
                    }]}}],
                    "usage": {"prompt_tokens": 10, "completion_tokens": 3}
                })
                .to_string(),
            })
            .unwrap();
        state
            .handle(SseEvent {
                event: None,
                data: "[DONE]".into(),
            })
            .unwrap();

        let events: Vec<_> = receiver.try_iter().collect();
        assert!(matches!(&events[0], ChatEvent::TextDelta(text) if text == "Hello "));
        let ChatEvent::Finished {
            tool_calls, usage, ..
        } = &events[1]
        else {
            panic!("expected finished event");
        };
        assert_eq!(tool_calls[0].name, "files__read");
        assert_eq!(tool_calls[0].arguments, "{\"path\":\"/tmp/a\"}");
        assert_eq!(usage.as_ref().unwrap().input_tokens, 10);
    }

    #[test]
    fn stream_state_finishes_on_end_without_done_marker() {
        let (sender, receiver) = mpsc::sync_channel(8);
        let mut state = StreamState::new(&sender);
        state
            .handle(SseEvent {
                event: None,
                data: json!({"choices": [{"delta": {"content": "ok"}}]}).to_string(),
            })
            .unwrap();
        state.emit_finished().unwrap();
        state.emit_finished().unwrap();
        let events: Vec<_> = receiver.try_iter().collect();
        assert_eq!(events.len(), 2);
        assert!(
            matches!(&events[1], ChatEvent::Finished { tool_calls, .. } if tool_calls.is_empty())
        );
    }
}
