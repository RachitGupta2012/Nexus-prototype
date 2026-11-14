//! Native Anthropic Messages API adapter (`POST /v1/messages`).
//!
//! Protocol facts used here, verified against Anthropic's published API and
//! Python SDK definitions:
//!
//! - Required headers: `x-api-key` (existing `XApiKey` auth mode) and
//!   `anthropic-version: 2023-06-01`.
//! - `max_tokens` is required; `system` is a top-level field, not a message.
//! - Messages use `user`/`assistant` roles; tool blocks are not implemented.
//! - Streaming uses named SSE events whose `data` JSON carries a `type`.
//!   Text arrives as `content_block_delta` with `delta.type == "text_delta"`.
//!   Usage arrives on `message_start` (input/cache tokens) and
//!   `message_delta` (cumulative output tokens).

use std::sync::mpsc::SyncSender;

use serde_json::{Value, json};
use tokio::sync::watch;

use super::http::{self, SseEvent};
use super::{
    ChatEvent, ChatRequest, FinishReason, ProviderError, ProviderSettings, send_event, status_error,
};
use crate::chat::{MAX_ASSISTANT_MESSAGE_BYTES, MessageRole, TokenUsage};

/// Anthropic requires this exact API version header on every request.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
/// `max_tokens` is mandatory. This default keeps requests valid when the UI has
/// no explicit output-token setting.
pub const DEFAULT_MAX_TOKENS: u32 = 4_096;
/// Upper bound used for local validation only; the API enforces model limits.
pub const MAX_MAX_TOKENS: u32 = 1_000_000;

const MAX_USAGE_EVENTS: usize = 10_000;

pub async fn stream(
    settings: &ProviderSettings,
    api_key: Option<&str>,
    request: &ChatRequest,
    sender: &SyncSender<ChatEvent>,
    mut cancel: watch::Receiver<bool>,
) -> Result<(), ProviderError> {
    let body = build_body(settings, request)?;
    let mut state = StreamState::new(sender);
    let headers = [("anthropic-version", ANTHROPIC_VERSION)];
    let result =
        http::post_sse_with_headers(settings, api_key, &body, &headers, &mut cancel, |event| {
            state.handle(event)
        })
        .await;
    match result {
        Ok(()) => {
            if !state.finished {
                state.emit_finished()?;
            }
            Ok(())
        }
        Err(ProviderError::HttpStatus(status)) => Err(status_error(status)),
        Err(error) => Err(error),
    }
}

fn build_body(settings: &ProviderSettings, request: &ChatRequest) -> Result<Value, ProviderError> {
    if !request.tools.is_empty() {
        return Err(ProviderError::InvalidConfig(
            "tool definitions are not implemented for Anthropic in this build",
        ));
    }

    let mut messages = Vec::new();
    let mut system = request.system_prompt.trim().to_owned();
    for message in &request.messages {
        if message.role == MessageRole::Tool || !message.tool_calls.is_empty() {
            return Err(ProviderError::InvalidConfig(
                "tool calls and tool results are not implemented for Anthropic in this build",
            ));
        }
        let content = &message.content;
        if message.role == MessageRole::System {
            // Preserve system content in Anthropic's top-level field rather
            // than dropping it or sending an unsupported role.
            if !content.trim().is_empty() {
                if !system.is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str(content);
            }
            continue;
        }
        // Empty user/assistant blocks are rejected by the API and carry no
        // content; order of the remaining turns is preserved.
        if content.trim().is_empty() {
            continue;
        }
        messages.push(json!({
            "role": message.role.as_str(),
            "content": content,
        }));
    }

    if messages.is_empty() {
        return Err(ProviderError::InvalidConfig(
            "Anthropic requires at least one non-empty user or assistant message",
        ));
    }

    let max_tokens = request.max_output_tokens.unwrap_or(DEFAULT_MAX_TOKENS);
    if max_tokens == 0 || max_tokens > MAX_MAX_TOKENS {
        return Err(ProviderError::InvalidConfig(
            "maximum output tokens must be between 1 and 1000000",
        ));
    }
    let mut body = json!({
        "model": settings.model.trim(),
        "max_tokens": max_tokens,
        "messages": messages,
        "stream": true,
    });
    if !system.is_empty() {
        body["system"] = json!(system);
    }
    if let Some(temperature) = request.temperature {
        if !temperature.is_finite() || !(0.0..=1.0).contains(&temperature) {
            return Err(ProviderError::InvalidConfig(
                "temperature must be between 0 and 1 for Anthropic",
            ));
        }
        body["temperature"] = json!(temperature);
    }
    Ok(body)
}

struct StreamState<'a> {
    sender: &'a SyncSender<ChatEvent>,
    text_bytes: usize,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    usage_events: usize,
    finished: bool,
}

impl<'a> StreamState<'a> {
    fn new(sender: &'a SyncSender<ChatEvent>) -> Self {
        Self {
            sender,
            text_bytes: 0,
            input_tokens: None,
            output_tokens: None,
            cached_input_tokens: None,
            usage_events: 0,
            finished: false,
        }
    }

    fn handle(&mut self, event: SseEvent) -> Result<(), ProviderError> {
        let data = event.data.trim();
        if data.is_empty() {
            return Ok(());
        }
        let value: Value =
            serde_json::from_str(data).map_err(|_| ProviderError::InvalidResponse)?;
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .or(event.event.as_deref())
            .unwrap_or_default();
        match kind {
            "message_start" => {
                if let Some(usage) = value.pointer("/message/usage") {
                    self.absorb_usage(usage)?;
                }
            }
            "content_block_start" => {
                if let Some(block) = value.get("content_block") {
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(text) = block.get("text").and_then(Value::as_str) {
                                self.emit_text(text)?;
                            }
                        }
                        Some("thinking") => {
                            if let Some(text) = block.get("thinking").and_then(Value::as_str) {
                                self.emit_reasoning(text)?;
                            }
                        }
                        _ => {}
                    }
                }
            }
            "content_block_delta" => {
                let delta = value.get("delta").ok_or(ProviderError::InvalidResponse)?;
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        let text = delta
                            .get("text")
                            .and_then(Value::as_str)
                            .ok_or(ProviderError::InvalidResponse)?;
                        self.emit_text(text)?;
                    }
                    Some("thinking_delta") => {
                        let text = delta
                            .get("thinking")
                            .and_then(Value::as_str)
                            .ok_or(ProviderError::InvalidResponse)?;
                        self.emit_reasoning(text)?;
                    }
                    // Signatures, partial JSON tool input, and citations are
                    // not generated text and are not part of this build's model.
                    Some("signature_delta" | "input_json_delta" | "citations_delta") => {}
                    Some(_) => {}
                    None => return Err(ProviderError::InvalidResponse),
                }
            }
            "content_block_stop" => {}
            "message_delta" => {
                if let Some(usage) = value.get("usage") {
                    self.absorb_usage(usage)?;
                }
            }
            "message_stop" => self.emit_finished()?,
            "ping" => {}
            "error" => return Err(ProviderError::RequestFailed),
            _ => {}
        }
        Ok(())
    }

    fn absorb_usage(&mut self, usage: &Value) -> Result<(), ProviderError> {
        self.usage_events = self.usage_events.saturating_add(1);
        if self.usage_events > MAX_USAGE_EVENTS {
            return Err(ProviderError::ResponseTooLarge);
        }
        if let Some(value) = usage.get("input_tokens").and_then(Value::as_u64) {
            self.input_tokens = Some(value);
        }
        if let Some(value) = usage.get("output_tokens").and_then(Value::as_u64) {
            self.output_tokens = Some(value);
        }
        if let Some(value) = usage.get("cache_read_input_tokens").and_then(Value::as_u64) {
            self.cached_input_tokens = Some(value);
        }
        Ok(())
    }

    fn emit_text(&mut self, text: &str) -> Result<(), ProviderError> {
        if text.is_empty() {
            return Ok(());
        }
        self.grow(text.len())?;
        send_event(self.sender, ChatEvent::TextDelta(text.to_owned()))
    }

    fn emit_reasoning(&mut self, text: &str) -> Result<(), ProviderError> {
        if text.is_empty() {
            return Ok(());
        }
        self.grow(text.len())?;
        send_event(self.sender, ChatEvent::ReasoningDelta(text.to_owned()))
    }

    fn grow(&mut self, bytes: usize) -> Result<(), ProviderError> {
        self.text_bytes = self.text_bytes.saturating_add(bytes);
        if self.text_bytes > MAX_ASSISTANT_MESSAGE_BYTES {
            return Err(ProviderError::ResponseTooLarge);
        }
        Ok(())
    }

    fn usage(&self) -> Option<TokenUsage> {
        let usage = TokenUsage {
            input_tokens: self.input_tokens.unwrap_or(0),
            output_tokens: self.output_tokens.unwrap_or(0),
            cached_input_tokens: self.cached_input_tokens.unwrap_or(0),
        };
        if usage.input_tokens == 0 && usage.output_tokens == 0 && usage.cached_input_tokens == 0 {
            None
        } else {
            Some(usage)
        }
    }

    fn emit_finished(&mut self) -> Result<(), ProviderError> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        send_event(
            self.sender,
            ChatEvent::Finished {
                tool_calls: Vec::new(),
                usage: self.usage(),
                finish: FinishReason::Completed,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{MessageStatus, StoredMessage};
    use crate::config::{AuthMode, Protocol};

    fn settings(base_url: &str) -> ProviderSettings {
        ProviderSettings {
            profile_id: 1,
            protocol: Protocol::AnthropicMessages,
            base_url: base_url.to_owned(),
            auth: AuthMode::XApiKey,
            model: "claude-sonnet-4-5".to_owned(),
        }
    }

    fn message(role: MessageRole, content: &str) -> StoredMessage {
        StoredMessage {
            id: 1,
            role,
            content: content.to_owned(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            tool_name: None,
            status: MessageStatus::Complete,
            created_at: 0,
            usage: None,
            cost: None,
        }
    }

    fn request(messages: Vec<StoredMessage>) -> ChatRequest {
        ChatRequest {
            system_prompt: "Be concise.".to_owned(),
            messages,
            tools: Vec::new(),
            temperature: None,
            max_output_tokens: None,
        }
    }

    #[test]
    fn builds_native_messages_body_with_top_level_system() {
        let body = build_body(
            &settings("https://api.anthropic.com/v1"),
            &request(vec![
                message(MessageRole::User, "hi"),
                message(MessageRole::Assistant, "hello"),
            ]),
        )
        .unwrap();
        assert_eq!(body["model"], "claude-sonnet-4-5");
        assert_eq!(body["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(body["stream"], true);
        assert_eq!(body["system"], "Be concise.");
        assert_eq!(
            body["messages"],
            json!([
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "hello"},
            ])
        );
        assert!(body.get("stream_options").is_none());
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn system_messages_are_preserved_in_the_top_level_field() {
        let body = build_body(
            &settings("https://api.anthropic.com/v1"),
            &request(vec![
                message(MessageRole::System, "Extra rule."),
                message(MessageRole::User, "hi"),
            ]),
        )
        .unwrap();
        assert_eq!(body["system"], "Be concise.\n\nExtra rule.");
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn keeps_message_order_and_skips_empty_blocks() {
        let body = build_body(
            &settings("https://api.anthropic.com/v1"),
            &request(vec![
                message(MessageRole::User, "one"),
                message(MessageRole::Assistant, "   "),
                message(MessageRole::User, "two"),
            ]),
        )
        .unwrap();
        assert_eq!(
            body["messages"],
            json!([
                {"role": "user", "content": "one"},
                {"role": "user", "content": "two"},
            ])
        );
    }

    #[test]
    fn maps_generation_settings_and_overrides_max_tokens() {
        let mut request = request(vec![message(MessageRole::User, "hi")]);
        request.temperature = Some(0.4);
        request.max_output_tokens = Some(1_024);
        let body = build_body(&settings("https://api.anthropic.com/v1"), &request).unwrap();
        assert!((body["temperature"].as_f64().unwrap() - 0.4).abs() < 1e-6);
        assert_eq!(body["max_tokens"], 1_024);
    }

    #[test]
    fn rejects_settings_outside_anthropic_ranges() {
        let mut request = request(vec![message(MessageRole::User, "hi")]);
        request.temperature = Some(1.5);
        assert!(matches!(
            build_body(&settings("https://api.anthropic.com/v1"), &request),
            Err(ProviderError::InvalidConfig(_))
        ));
        request.temperature = Some(f32::NAN);
        assert!(matches!(
            build_body(&settings("https://api.anthropic.com/v1"), &request),
            Err(ProviderError::InvalidConfig(_))
        ));
        request.temperature = None;
        for tokens in [0, MAX_MAX_TOKENS + 1] {
            request.max_output_tokens = Some(tokens);
            assert!(matches!(
                build_body(&settings("https://api.anthropic.com/v1"), &request),
                Err(ProviderError::InvalidConfig(_))
            ));
        }
    }

    #[test]
    fn rejects_tool_content_instead_of_discarding_it() {
        let mut with_tools = request(vec![message(MessageRole::User, "hi")]);
        with_tools.tools.push(crate::provider::ToolDefinition {
            name: "lookup".to_owned(),
            description: String::new(),
            parameters: json!({"type": "object"}),
        });
        assert!(matches!(
            build_body(&settings("https://api.anthropic.com/v1"), &with_tools),
            Err(ProviderError::InvalidConfig(_))
        ));

        let mut with_tool_call = message(MessageRole::Assistant, "checking");
        with_tool_call.tool_calls.push(crate::chat::ToolCall {
            id: "call_1".to_owned(),
            name: "lookup".to_owned(),
            arguments: "{}".to_owned(),
        });
        assert!(matches!(
            build_body(
                &settings("https://api.anthropic.com/v1"),
                &request(vec![with_tool_call])
            ),
            Err(ProviderError::InvalidConfig(_))
        ));

        assert!(matches!(
            build_body(
                &settings("https://api.anthropic.com/v1"),
                &request(vec![message(MessageRole::Tool, "result")])
            ),
            Err(ProviderError::InvalidConfig(_))
        ));
    }

    #[test]
    fn rejects_requests_without_any_sendable_message() {
        assert!(matches!(
            build_body(
                &settings("https://api.anthropic.com/v1"),
                &request(vec![message(MessageRole::User, " ")])
            ),
            Err(ProviderError::InvalidConfig(_))
        ));
    }

    #[test]
    fn uses_the_messages_endpoint_and_supports_custom_prefixes() {
        assert_eq!(
            settings("https://api.anthropic.com/v1")
                .endpoint()
                .unwrap()
                .as_str(),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            settings("https://gateway.example.test/anthropic/v1/")
                .endpoint()
                .unwrap()
                .as_str(),
            "https://gateway.example.test/anthropic/v1/messages"
        );
    }

    fn decode(chunks: &[&[u8]]) -> (Vec<ChatEvent>, bool) {
        let (sender, receiver) = std::sync::mpsc::sync_channel(256);
        let mut state = StreamState::new(&sender);
        let mut decoder = http::SseDecoder::default();
        for chunk in chunks {
            for event in decoder.push(chunk).unwrap() {
                state.handle(event).unwrap();
            }
        }
        for event in decoder.finish().unwrap() {
            state.handle(event).unwrap();
        }
        if !state.finished {
            state.emit_finished().unwrap();
        }
        let mut events = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            events.push(event);
        }
        (events, state.finished)
    }

    fn text_of(events: &[ChatEvent]) -> String {
        events
            .iter()
            .filter_map(|event| match event {
                ChatEvent::TextDelta(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn parses_named_sse_text_deltas_across_arbitrary_chunks() {
        let stream = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":11,\"output_tokens\":1}}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: ping\ndata: {\"type\":\"ping\"}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":7}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        );
        let bytes = stream.as_bytes();
        let chunks: Vec<&[u8]> = bytes.chunks(17).collect();
        let (events, finished) = decode(&chunks);
        assert!(finished);
        assert_eq!(text_of(&events), "Hello");
        match events.last().unwrap() {
            ChatEvent::Finished {
                usage, tool_calls, ..
            } => {
                assert!(tool_calls.is_empty());
                assert_eq!(
                    usage,
                    &Some(TokenUsage {
                        input_tokens: 11,
                        output_tokens: 7,
                        cached_input_tokens: 0
                    })
                );
            }
            other => panic!("unexpected final event: {other:?}"),
        }
    }

    #[test]
    fn multiple_events_in_one_chunk_keep_order() {
        let (events, _) = decode(&[
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"a\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"b\"}}\n\n",
        ]);
        assert_eq!(text_of(&events), "ab");
    }

    #[test]
    fn thinking_and_unknown_deltas_do_not_become_text() {
        let (events, _) = decode(&[
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"plan\"}}\n\n",
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig\"}}\n\n",
            b"event: mystery\ndata: {\"type\":\"mystery\",\"value\":1}\n\n",
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n",
        ]);
        assert_eq!(text_of(&events), "ok");
        assert!(matches!(
            events[0],
            ChatEvent::ReasoningDelta(ref text) if text == "plan"
        ));
    }

    #[test]
    fn malformed_payloads_fail_instead_of_panicking() {
        let (sender, _receiver) = std::sync::mpsc::sync_channel(8);
        let mut state = StreamState::new(&sender);
        assert_eq!(
            state.handle(SseEvent {
                event: Some("content_block_delta".to_owned()),
                data: "not json".to_owned()
            }),
            Err(ProviderError::InvalidResponse)
        );
    }

    #[test]
    fn error_events_fail_without_exposing_the_message() {
        let (sender, _receiver) = std::sync::mpsc::sync_channel(8);
        let mut state = StreamState::new(&sender);
        let error = state
            .handle(SseEvent {
                event: Some("error".to_owned()),
                data: r#"{"type":"error","error":{"type":"overloaded_error","message":"secret prompt text"}}"#.to_owned(),
            })
            .unwrap_err();
        assert_eq!(error, ProviderError::RequestFailed);
        assert!(!format!("{error:?}").contains("secret"));
    }

    #[test]
    fn incomplete_streams_still_finish_without_losing_text() {
        let (events, finished) = decode(&[
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}\n\n",
        ]);
        assert!(finished);
        assert_eq!(text_of(&events), "partial");
        assert!(matches!(
            events.last().unwrap(),
            ChatEvent::Finished { usage: None, .. }
        ));
    }

    #[test]
    fn statuses_map_to_sanitized_errors() {
        assert_eq!(status_error(400), ProviderError::InvalidRequest);
        assert_eq!(status_error(401), ProviderError::AuthenticationRejected);
        assert_eq!(status_error(403), ProviderError::PermissionDenied);
        assert_eq!(status_error(404), ProviderError::ModelNotFound);
        assert_eq!(status_error(429), ProviderError::RateLimited);
        assert_eq!(status_error(500), ProviderError::HttpStatus(500));
        assert_eq!(status_error(529), ProviderError::HttpStatus(529));
    }

    // ---- loopback HTTP integration -------------------------------------

    use std::io::{Read, Write};

    /// Serves one HTTP response; returns the port and the request head.
    fn serve_once(
        content_type: &'static str,
        body: &'static str,
    ) -> (u16, std::sync::mpsc::Receiver<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0u8; 8192];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let _ = sender.send(String::from_utf8_lossy(&buffer[..read]).to_string());
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (port, receiver)
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    fn run_stream(base: &str, request: ChatRequest) -> (Result<(), ProviderError>, Vec<ChatEvent>) {
        let (sender, receiver) = std::sync::mpsc::sync_channel(64);
        let (_cancel_sender, cancel) = tokio::sync::watch::channel(false);
        let result = block_on(stream(
            &settings(base),
            Some("test-key"),
            &request,
            &sender,
            cancel,
        ));
        drop(sender);
        let mut events = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            events.push(event);
        }
        (result, events)
    }

    #[test]
    fn sends_required_headers_and_streams_text() {
        let body = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":3,\"output_tokens\":0}}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        );
        let (port, request_head) = serve_once("text/event-stream", body);
        let (result, events) = run_stream(
            &format!("http://127.0.0.1:{port}/v1"),
            request(vec![message(MessageRole::User, "hi")]),
        );
        assert_eq!(result, Ok(()));
        assert_eq!(text_of(&events), "Hello");
        let head = request_head.recv().unwrap().to_lowercase();
        assert!(head.starts_with("post /v1/messages "), "{head}");
        assert!(head.contains("anthropic-version: 2023-06-01"), "{head}");
        assert!(head.contains("x-api-key: test-key"), "{head}");
        assert!(!head.contains("authorization:"), "{head}");
    }

    #[test]
    fn http_errors_are_mapped_without_reading_bodies() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer);
                let _ = stream.write_all(
                    b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });
        let (result, events) = run_stream(
            &format!("http://127.0.0.1:{port}/v1"),
            request(vec![message(MessageRole::User, "hi")]),
        );
        assert_eq!(result, Err(ProviderError::RateLimited));
        assert!(events.is_empty());
    }

    #[test]
    fn cancellation_stops_an_open_stream() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer);
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                );
                let _ = stream.flush();
                // One delta, then stall so cancellation has time to win.
                let _ = stream.write_all(
                    b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"one\"}}\n\n",
                );
                let _ = stream.flush();
                std::thread::sleep(std::time::Duration::from_secs(5));
            }
        });
        let (sender, receiver) = std::sync::mpsc::sync_channel(64);
        let (cancel_sender, cancel) = tokio::sync::watch::channel(false);
        let base = format!("http://127.0.0.1:{port}/v1");
        let handle = std::thread::spawn(move || {
            block_on(stream(
                &settings(&base),
                Some("test-key"),
                &request(vec![message(MessageRole::User, "hi")]),
                &sender,
                cancel,
            ))
        });
        let first = receiver
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        assert!(matches!(first, ChatEvent::TextDelta(ref text) if text == "one"));
        cancel_sender.send(true).unwrap();
        let result = handle.join().unwrap();
        assert_eq!(result, Err(ProviderError::Cancelled));
    }

    #[test]
    fn controller_dispatches_anthropic_requests() {
        let body = concat!(
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"via-controller\"}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        );
        let (port, _request) = serve_once("text/event-stream", body);
        let mut controller = super::super::ProviderController::start(
            settings(&format!("http://127.0.0.1:{port}/v1")),
            Some("test-key".to_owned()),
            request(vec![message(MessageRole::User, "hi")]),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut text = String::new();
        let mut finished = false;
        while std::time::Instant::now() < deadline {
            for event in controller.poll() {
                match event {
                    ChatEvent::TextDelta(delta) => text.push_str(&delta),
                    ChatEvent::Finished { .. } => finished = true,
                    ChatEvent::Failed(error) => panic!("provider error: {error}"),
                    ChatEvent::ReasoningDelta(_) => {}
                }
            }
            if finished {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(finished, "controller never finished");
        assert_eq!(text, "via-controller");
    }

    #[test]
    fn controller_rejects_unsupported_tool_requests_before_network() {
        let mut request = request(vec![message(MessageRole::User, "hi")]);
        request.tools.push(crate::provider::ToolDefinition {
            name: "lookup".to_owned(),
            description: String::new(),
            parameters: json!({"type": "object"}),
        });
        let mut controller = super::super::ProviderController::start(
            settings("https://api.anthropic.com/v1"),
            Some("test-key".to_owned()),
            request,
        )
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Some(event) = controller.poll().into_iter().next() {
                match event {
                    ChatEvent::Failed(ProviderError::InvalidConfig(_)) => return,
                    other => panic!("unexpected event: {other:?}"),
                }
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
