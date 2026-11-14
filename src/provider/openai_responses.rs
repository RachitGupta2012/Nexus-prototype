//! Native OpenAI Responses API adapter (`POST /v1/responses`).
//!
//! Protocol facts used here, verified against OpenAI's published Responses
//! reference:
//!
//! - `POST {base_url}/responses` with `Authorization: Bearer` (existing
//!   `Bearer` auth mode) and `stream: true`.
//! - Input is the Responses `input` array, not Chat Completions `messages`.
//!   System/developer guidance is sent through top-level `instructions`.
//! - Streaming is SSE whose events are named `response.*`; the JSON `data`
//!   object also carries `type`. Text arrives in `response.output_text.delta`.
//!   `response.output_text.done` repeats the complete text and is not appended
//!   again.
//! - Reasoning arrives in `response.reasoning_summary_text.delta` /
//!   `response.reasoning_text.delta` and is kept out of assistant text.
//! - Terminal events are `response.completed`, `response.incomplete`, and
//!   `response.failed`; `response.incomplete_details.reason` distinguishes
//!   token-limit from other stops.
//! - Usage is on the terminal response object: `usage.input_tokens`,
//!   `usage.output_tokens`, `usage.input_tokens_details.cached_tokens`.
//! - Model listing remains `GET /models`; the published endpoint returns the
//!   full list in one response, so no pagination is applied.
//!
//! Documentation: <https://developers.openai.com/api/reference/resources/responses/methods/create>
//! and <https://developers.openai.com/api/reference/resources/responses/streaming-events>.

use std::sync::mpsc::SyncSender;

use serde_json::{Value, json};
use tokio::sync::watch;

use super::http::{self, SseEvent};
use super::{
    ChatEvent, ChatRequest, FinishReason, ProviderError, ProviderSettings, send_event, status_error,
};
use crate::chat::{MAX_ASSISTANT_MESSAGE_BYTES, MessageRole, TokenUsage};

/// Local bound only; the API enforces per-model output limits.
pub const MAX_OUTPUT_TOKENS: u32 = 1_000_000;
/// Responses accepts temperatures from 0 to 2.
pub const MAX_TEMPERATURE: f32 = 2.0;

pub async fn stream(
    settings: &ProviderSettings,
    api_key: Option<&str>,
    request: &ChatRequest,
    sender: &SyncSender<ChatEvent>,
    mut cancel: watch::Receiver<bool>,
) -> Result<(), ProviderError> {
    let body = build_body(settings, request)?;
    let mut state = StreamState::new(sender);
    let result = http::post_sse(settings, api_key, &body, &mut cancel, |event| {
        state.handle(event)
    })
    .await;
    match result {
        Ok(()) => state.emit_finished(),
        Err(ProviderError::HttpStatus(status)) => Err(status_error(status)),
        Err(error) => Err(error),
    }
}

/// Builds a native Responses request. Chat Completions fields (`messages`,
/// `max_tokens`) are never sent.
fn build_body(settings: &ProviderSettings, request: &ChatRequest) -> Result<Value, ProviderError> {
    if !request.tools.is_empty() {
        return Err(ProviderError::InvalidConfig(
            "tool definitions are not implemented for OpenAI Responses in this build",
        ));
    }

    let mut instructions = request.system_prompt.trim().to_owned();
    let mut input = Vec::new();
    for message in &request.messages {
        if message.role == MessageRole::Tool || !message.tool_calls.is_empty() {
            return Err(ProviderError::InvalidConfig(
                "tool calls and tool results are not implemented for OpenAI Responses in this build",
            ));
        }
        if message.role == MessageRole::System {
            if !message.content.trim().is_empty() {
                if !instructions.is_empty() {
                    instructions.push_str("\n\n");
                }
                instructions.push_str(&message.content);
            }
            continue;
        }
        if message.content.trim().is_empty() {
            continue;
        }
        let role = match message.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::System | MessageRole::Tool => unreachable!(),
        };
        input.push(json!({
            "role": role,
            "content": message.content,
        }));
    }

    if input.is_empty() {
        return Err(ProviderError::InvalidConfig(
            "OpenAI Responses requires at least one non-empty user or assistant message",
        ));
    }

    let mut body = json!({
        "model": settings.model.trim(),
        "input": input,
        "stream": true,
        // Responses are stored by default at the provider. NEXUS never needs
        // retrieval, so opt out explicitly.
        "store": false,
    });
    if !instructions.is_empty() {
        body["instructions"] = json!(instructions);
    }
    if let Some(temperature) = request.temperature {
        if !temperature.is_finite() || !(0.0..=MAX_TEMPERATURE).contains(&temperature) {
            return Err(ProviderError::InvalidConfig(
                "temperature must be between 0 and 2 for OpenAI Responses",
            ));
        }
        body["temperature"] = json!(temperature);
    }
    if let Some(max_tokens) = request.max_output_tokens {
        if max_tokens == 0 || max_tokens > MAX_OUTPUT_TOKENS {
            return Err(ProviderError::InvalidConfig(
                "maximum output tokens must be between 1 and 1000000",
            ));
        }
        body["max_output_tokens"] = json!(max_tokens);
    }
    Ok(body)
}

struct StreamState<'a> {
    sender: &'a SyncSender<ChatEvent>,
    text_bytes: usize,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    finish: Option<FinishReason>,
    refused: bool,
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
            finish: None,
            refused: false,
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
            "response.output_text.delta" => {
                let delta = value
                    .get("delta")
                    .and_then(Value::as_str)
                    .ok_or(ProviderError::InvalidResponse)?;
                self.emit_text(delta)?;
            }
            "response.refusal.delta" => {
                self.refused = true;
                let delta = value
                    .get("delta")
                    .and_then(Value::as_str)
                    .ok_or(ProviderError::InvalidResponse)?;
                self.emit_text(delta)?;
            }
            "response.refusal.done" => {
                self.refused = true;
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let delta = value
                    .get("delta")
                    .and_then(Value::as_str)
                    .ok_or(ProviderError::InvalidResponse)?;
                self.emit_reasoning(delta)?;
            }
            "response.output_item.added" | "response.output_item.done" => {
                if let Some(item) = value.get("item") {
                    check_output_item(item)?;
                }
            }
            "response.content_part.added" | "response.content_part.done" => {
                if let Some(part) = value.get("part") {
                    check_content_part(part)?;
                }
            }
            "response.completed" => {
                let response = value
                    .get("response")
                    .ok_or(ProviderError::InvalidResponse)?;
                self.absorb_response(response)?;
                let finish = if self.refused {
                    FinishReason::Blocked
                } else {
                    match response.get("status").and_then(Value::as_str) {
                        Some("completed") => FinishReason::Completed,
                        Some("incomplete") => self.incomplete_reason(response),
                        Some("failed") => return Err(ProviderError::RequestFailed),
                        _ => FinishReason::Incomplete,
                    }
                };
                self.finish = Some(finish);
                self.emit_finished()?;
            }
            "response.incomplete" => {
                let response = value
                    .get("response")
                    .ok_or(ProviderError::InvalidResponse)?;
                self.absorb_response(response)?;
                self.finish = Some(if self.refused {
                    FinishReason::Blocked
                } else {
                    self.incomplete_reason(response)
                });
                self.emit_finished()?;
            }
            "response.failed" => return Err(ProviderError::RequestFailed),
            "error" => return Err(ProviderError::RequestFailed),
            // Created/in-progress markers, completion echoes
            // (`response.output_text.done`, `response.content_part.done`,
            // `response.output_item.done`, reasoning done/part events), and
            // unknown event types carry no new user-facing text.
            _ => {}
        }
        Ok(())
    }

    fn incomplete_reason(&self, response: &Value) -> FinishReason {
        match response
            .pointer("/incomplete_details/reason")
            .and_then(Value::as_str)
        {
            Some("max_output_tokens") => FinishReason::MaxTokens,
            Some("content_filter") => FinishReason::Blocked,
            _ => FinishReason::Incomplete,
        }
    }

    fn absorb_response(&mut self, response: &Value) -> Result<(), ProviderError> {
        if let Some(output) = response.get("output").and_then(Value::as_array) {
            for item in output {
                check_output_item(item)?;
                // Reasoning items carry summaries, not message content parts.
                if item.get("type").and_then(Value::as_str) != Some("message") {
                    continue;
                }
                if let Some(parts) = item.get("content").and_then(Value::as_array) {
                    for part in parts {
                        check_content_part(part)?;
                        if part.get("type").and_then(Value::as_str) == Some("refusal") {
                            self.refused = true;
                        }
                    }
                }
            }
        }
        if let Some(usage) = response.get("usage") {
            if let Some(value) = usage.get("input_tokens").and_then(Value::as_u64) {
                self.input_tokens = Some(value);
            }
            if let Some(value) = usage.get("output_tokens").and_then(Value::as_u64) {
                self.output_tokens = Some(value);
            }
            if let Some(value) = usage
                .pointer("/input_tokens_details/cached_tokens")
                .and_then(Value::as_u64)
            {
                self.cached_input_tokens = Some(value);
            }
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
        let finish = if self.refused {
            FinishReason::Blocked
        } else {
            self.finish.unwrap_or(FinishReason::Incomplete)
        };
        send_event(
            self.sender,
            ChatEvent::Finished {
                tool_calls: Vec::new(),
                usage: self.usage(),
                finish,
            },
        )
    }
}

fn check_output_item(item: &Value) -> Result<(), ProviderError> {
    match item.get("type").and_then(Value::as_str) {
        Some("message" | "reasoning") | None => Ok(()),
        Some(_) => Err(ProviderError::UnsupportedOperation(
            "tool calls and non-text output items are not implemented",
        )),
    }
}

fn check_content_part(part: &Value) -> Result<(), ProviderError> {
    match part.get("type").and_then(Value::as_str) {
        Some("output_text" | "refusal") | None => Ok(()),
        Some(_) => Err(ProviderError::UnsupportedOperation(
            "non-text content parts are not implemented",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{MessageStatus, StoredMessage};
    use crate::config::{AuthMode, Protocol};
    use crate::provider::ProviderController;
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    fn settings(base_url: &str) -> ProviderSettings {
        ProviderSettings {
            profile_id: 1,
            protocol: Protocol::OpenAiResponses,
            base_url: base_url.to_owned(),
            auth: AuthMode::Bearer,
            model: "gpt-4.1".to_owned(),
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
            system_prompt: String::new(),
            messages,
            tools: Vec::new(),
            temperature: None,
            max_output_tokens: None,
        }
    }

    fn body(req: &ChatRequest) -> Value {
        build_body(&settings("https://api.openai.com/v1"), req).expect("body")
    }

    fn sse(data: &str) -> SseEvent {
        SseEvent {
            event: None,
            data: data.to_owned(),
        }
    }

    fn delta(text: &str) -> SseEvent {
        sse(&format!(
            "{{\"type\":\"response.output_text.delta\",\"delta\":{}}}",
            json!(text)
        ))
    }

    fn completed() -> SseEvent {
        sse(
            r#"{"type":"response.completed","response":{"status":"completed","output":[],"usage":{"input_tokens":5,"output_tokens":7,"input_tokens_details":{"cached_tokens":2}}}}"#,
        )
    }

    /// Serves one fixed HTTP response and records the request head.
    fn serve_once(response: String, bodies: Arc<Mutex<Vec<String>>>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).unwrap_or(0);
            bodies
                .lock()
                .expect("lock")
                .push(String::from_utf8_lossy(&buf[..n]).into_owned());
            let _ = stream.write_all(response.as_bytes());
        });
        format!("http://{addr}/v1")
    }

    #[test]
    fn request_uses_native_input_instructions_and_settings() {
        let mut req = request(vec![
            message(MessageRole::System, "be brief"),
            message(MessageRole::User, "hello"),
            message(MessageRole::Assistant, "hi"),
            message(MessageRole::User, "again"),
        ]);
        req.system_prompt = "top system".to_owned();
        req.temperature = Some(0.3);
        req.max_output_tokens = Some(99);
        let json = body(&req);
        assert_eq!(json["model"], "gpt-4.1");
        assert_eq!(json["input"][0]["role"], "user");
        assert_eq!(json["input"][0]["content"], "hello");
        assert_eq!(json["input"][1]["role"], "assistant");
        assert_eq!(json["input"][2]["content"], "again");
        assert_eq!(json["instructions"], "top system\n\nbe brief");
        assert_eq!(json["stream"], true);
        assert_eq!(json["store"], false);
        let temperature = json["temperature"].as_f64().expect("temperature");
        assert!((temperature - 0.3).abs() < 1e-6);
        assert_eq!(json["max_output_tokens"], 99);
        assert!(json.get("messages").is_none());
        assert!(json.get("max_tokens").is_none());
        assert!(json.get("system").is_none());
        assert!(json.get("input_text").is_none());
    }

    #[test]
    fn empty_messages_are_skipped_and_order_is_preserved() {
        let json = body(&request(vec![
            message(MessageRole::User, "one"),
            message(MessageRole::User, "   "),
            message(MessageRole::Assistant, "two"),
            message(MessageRole::User, "three"),
        ]));
        let input = json["input"].as_array().expect("array");
        assert_eq!(input.len(), 3);
        assert_eq!(input[0]["content"], "one");
        assert_eq!(input[1]["role"], "assistant");
        assert_eq!(input[2]["content"], "three");
    }

    #[test]
    fn consecutive_same_role_turns_are_preserved() {
        let json = body(&request(vec![
            message(MessageRole::User, "one"),
            message(MessageRole::User, "two"),
        ]));
        assert_eq!(json["input"].as_array().expect("array").len(), 2);
    }

    #[test]
    fn tool_content_is_rejected_explicitly() {
        let mut req = request(vec![message(MessageRole::Tool, "result")]);
        assert!(matches!(
            build_body(&settings("https://example.com/v1"), &req),
            Err(ProviderError::InvalidConfig(_))
        ));
        req.messages = vec![message(MessageRole::User, "hi")];
        req.messages[0].tool_calls = vec![crate::provider::ToolCall {
            id: "1".to_owned(),
            name: "x".to_owned(),
            arguments: "{}".to_owned(),
        }];
        assert!(build_body(&settings("https://example.com/v1"), &req).is_err());
    }

    #[test]
    fn temperature_and_output_token_bounds_are_enforced() {
        let mut req = request(vec![message(MessageRole::User, "hi")]);
        req.temperature = Some(2.5);
        assert!(build_body(&settings("https://example.com/v1"), &req).is_err());
        req.temperature = Some(2.0);
        assert!(build_body(&settings("https://example.com/v1"), &req).is_ok());
        req.temperature = None;
        req.max_output_tokens = Some(0);
        assert!(build_body(&settings("https://example.com/v1"), &req).is_err());
        req.max_output_tokens = Some(MAX_OUTPUT_TOKENS + 1);
        assert!(build_body(&settings("https://example.com/v1"), &req).is_err());
    }

    #[test]
    fn endpoint_supports_custom_prefixes_without_duplicate_segments() {
        assert_eq!(
            settings("https://api.openai.com/v1")
                .endpoint()
                .expect("url")
                .as_str(),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(
            settings("https://gateway.example.com/openai/v1")
                .endpoint()
                .expect("url")
                .as_str(),
            "https://gateway.example.com/openai/v1/responses"
        );
    }

    #[test]
    fn text_deltas_are_ordered_and_done_text_is_not_duplicated() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(32);
        let mut state = StreamState::new(&sender);
        state.handle(delta("Hel")).expect("ok");
        state.handle(delta("lo")).expect("ok");
        state
            .handle(sse(
                r#"{"type":"response.output_text.done","text":"Hello"}"#,
            ))
            .expect("ok");
        state.handle(completed()).expect("ok");
        drop(sender);
        let events: Vec<_> = receiver.try_iter().collect();
        let text: String = events
            .iter()
            .filter_map(|event| match event {
                ChatEvent::TextDelta(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "Hello");
        let Some(ChatEvent::Finished {
            usage: Some(usage),
            finish,
            ..
        }) = events.last()
        else {
            panic!("missing finished event");
        };
        assert_eq!(*finish, FinishReason::Completed);
        assert_eq!(usage.input_tokens, 5);
        assert_eq!(usage.output_tokens, 7);
        assert_eq!(usage.cached_input_tokens, 2);
    }

    #[test]
    fn split_events_and_multiple_parts_keep_order() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(32);
        let mut state = StreamState::new(&sender);
        let wire = "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"A\"}\n\nevent: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"B\"}\n\n";
        let mut decoder = http::SseDecoder::default();
        for chunk in wire.as_bytes().chunks(9) {
            for event in decoder.push(chunk).expect("decode") {
                state.handle(event).expect("ok");
            }
        }
        for event in decoder.finish().expect("flush") {
            state.handle(event).expect("ok");
        }
        drop(sender);
        let text: String = receiver
            .try_iter()
            .filter_map(|event| match event {
                ChatEvent::TextDelta(text) => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(text, "AB");
    }

    #[test]
    fn multiple_output_items_and_content_parts_are_all_emitted_in_order() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(32);
        let mut state = StreamState::new(&sender);
        state
            .handle(sse(
                r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"message","role":"assistant","content":[]}}"#,
            ))
            .expect("ok");
        state
            .handle(sse(
                r#"{"type":"response.content_part.added","item_id":"m1","content_index":0,"part":{"type":"output_text","text":""}}"#,
            ))
            .expect("ok");
        state.handle(delta("first")).expect("ok");
        state
            .handle(sse(
                r#"{"type":"response.output_item.added","output_index":1,"item":{"type":"message","role":"assistant","content":[]}}"#,
            ))
            .expect("ok");
        state
            .handle(sse(
                r#"{"type":"response.content_part.added","item_id":"m2","content_index":0,"part":{"type":"output_text","text":""}}"#,
            ))
            .expect("ok");
        state.handle(delta("second")).expect("ok");
        drop(sender);
        let text: String = receiver
            .try_iter()
            .filter_map(|event| match event {
                ChatEvent::TextDelta(text) => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(text, "firstsecond");
    }

    #[test]
    fn reasoning_deltas_stay_out_of_assistant_text() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(32);
        let mut state = StreamState::new(&sender);
        state
            .handle(sse(
                r#"{"type":"response.reasoning_summary_text.delta","delta":"plan"}"#,
            ))
            .expect("ok");
        state
            .handle(sse(
                r#"{"type":"response.reasoning_text.delta","delta":"more"}"#,
            ))
            .expect("ok");
        state.handle(delta("answer")).expect("ok");
        drop(sender);
        let events: Vec<_> = receiver.try_iter().collect();
        let reasoning: String = events
            .iter()
            .filter_map(|event| match event {
                ChatEvent::ReasoningDelta(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let text: String = events
            .iter()
            .filter_map(|event| match event {
                ChatEvent::TextDelta(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(reasoning, "planmore");
        assert_eq!(text, "answer");
    }

    #[test]
    fn failed_and_error_events_fail_the_stream() {
        let (sender, _receiver) = std::sync::mpsc::sync_channel(8);
        let mut state = StreamState::new(&sender);
        assert!(matches!(
            state.handle(sse(
                r#"{"type":"response.failed","response":{"status":"failed","error":{"code":"server_error"}}}"#
            )),
            Err(ProviderError::RequestFailed)
        ));
        assert!(matches!(
            state.handle(sse(r#"{"type":"error","message":"boom"}"#)),
            Err(ProviderError::RequestFailed)
        ));
    }

    #[test]
    fn malformed_events_and_missing_deltas_are_invalid_responses() {
        let (sender, _receiver) = std::sync::mpsc::sync_channel(8);
        let mut state = StreamState::new(&sender);
        assert!(matches!(
            state.handle(sse("{not json")),
            Err(ProviderError::InvalidResponse)
        ));
        assert!(matches!(
            state.handle(sse(r#"{"type":"response.output_text.delta"}"#)),
            Err(ProviderError::InvalidResponse)
        ));
    }

    #[test]
    fn unknown_events_are_ignored() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(8);
        let mut state = StreamState::new(&sender);
        state
            .handle(sse(r#"{"type":"response.something_new","value":1}"#))
            .expect("ignored");
        drop(sender);
        assert_eq!(receiver.try_iter().count(), 0);
    }

    #[test]
    fn incomplete_max_tokens_and_content_filter_map_distinctly() {
        for (reason, expected) in [
            ("max_output_tokens", FinishReason::MaxTokens),
            ("content_filter", FinishReason::Blocked),
            ("other", FinishReason::Incomplete),
        ] {
            let (sender, receiver) = std::sync::mpsc::sync_channel(8);
            let mut state = StreamState::new(&sender);
            state
                .handle(sse(&format!(
                    "{{\"type\":\"response.incomplete\",\"response\":{{\"status\":\"incomplete\",\"incomplete_details\":{{\"reason\":\"{reason}\"}},\"output\":[]}}}}"
                )))
                .expect("ok");
            drop(sender);
            let Some(ChatEvent::Finished { finish, .. }) = receiver.try_iter().last() else {
                panic!("missing finish");
            };
            assert_eq!(finish, expected, "reason {reason}");
        }
    }

    #[test]
    fn refusal_and_tool_output_are_not_silent_successes() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(8);
        let mut state = StreamState::new(&sender);
        state
            .handle(sse(
                r#"{"type":"response.refusal.delta","delta":"I cannot help with that."}"#,
            ))
            .expect("ok");
        state.handle(completed()).expect("ok");
        drop(sender);
        let events: Vec<_> = receiver.try_iter().collect();
        assert!(matches!(
            events.last(),
            Some(ChatEvent::Finished {
                finish: FinishReason::Blocked,
                ..
            })
        ));

        let (sender, _receiver) = std::sync::mpsc::sync_channel(8);
        let mut state = StreamState::new(&sender);
        assert!(matches!(
            state.handle(sse(
                r#"{"type":"response.output_item.added","item":{"type":"function_call","name":"x"}}"#
            )),
            Err(ProviderError::UnsupportedOperation(_))
        ));
    }

    #[test]
    fn missing_terminal_event_emits_incomplete() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(8);
        let mut state = StreamState::new(&sender);
        state.handle(delta("partial")).expect("ok");
        state.emit_finished().expect("finish");
        drop(sender);
        assert!(matches!(
            receiver.try_iter().last(),
            Some(ChatEvent::Finished {
                finish: FinishReason::Incomplete,
                ..
            })
        ));
    }

    #[test]
    fn loopback_stream_extracts_text_and_sends_bearer_without_echoing_key() {
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let response = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n\
event: response.created\ndata: {\"type\":\"response.created\"}\n\n\
event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Hi\"}\n\n\
event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\" there\"}\n\n\
event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hi there\"}]}],\"usage\":{\"input_tokens\":2,\"output_tokens\":2}}}\n\n";
        let base = serve_once(response.to_owned(), bodies.clone());
        let mut controller = ProviderController::start(
            settings(&base),
            Some("secret-key".to_owned()),
            request(vec![message(MessageRole::User, "hi")]),
        )
        .expect("start");
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut events = Vec::new();
        while Instant::now() < deadline {
            events.extend(controller.poll());
            if events
                .iter()
                .any(|event| matches!(event, ChatEvent::Finished { .. }))
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let text: String = events
            .iter()
            .filter_map(|event| match event {
                ChatEvent::TextDelta(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "Hi there");
        assert!(matches!(
            events.last(),
            Some(ChatEvent::Finished {
                finish: FinishReason::Completed,
                usage: Some(_),
                ..
            })
        ));
        let requests = bodies.lock().expect("lock");
        let request = requests.first().expect("request");
        assert!(request.starts_with("POST /v1/responses "));
        let lower = request.to_lowercase();
        assert!(lower.contains("authorization: bearer secret-key"));
        assert!(lower.contains("\"store\":false"));
        assert!(!format!("{events:?}").contains("secret-key"));
    }

    #[test]
    fn http_statuses_map_to_sanitized_errors() {
        for (status, expected) in [
            (400, ProviderError::InvalidRequest),
            (401, ProviderError::AuthenticationRejected),
            (403, ProviderError::PermissionDenied),
            (404, ProviderError::ModelNotFound),
            (429, ProviderError::RateLimited),
        ] {
            let bodies = Arc::new(Mutex::new(Vec::new()));
            let response = format!(
                "HTTP/1.1 {status} Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let base = serve_once(response, bodies);
            let mut controller = ProviderController::start(
                settings(&base),
                Some("key".to_owned()),
                request(vec![message(MessageRole::User, "hi")]),
            )
            .expect("start");
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut failure = None;
            while Instant::now() < deadline {
                for event in controller.poll() {
                    if let ChatEvent::Failed(error) = event {
                        failure = Some(error);
                    }
                }
                if failure.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(failure, Some(expected), "status {status}");
        }
    }

    #[test]
    fn cancellation_stops_an_open_stream() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            );
            std::thread::sleep(Duration::from_secs(2));
        });
        let mut controller = ProviderController::start(
            settings(&format!("http://{addr}/v1")),
            Some("key".to_owned()),
            request(vec![message(MessageRole::User, "hi")]),
        )
        .expect("start");
        std::thread::sleep(Duration::from_millis(100));
        controller.cancel();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut cancelled = false;
        while Instant::now() < deadline {
            for event in controller.poll() {
                if matches!(event, ChatEvent::Failed(ProviderError::Cancelled)) {
                    cancelled = true;
                }
            }
            if cancelled {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(cancelled, "expected cancellation");
    }
}
