//! Native Google Gemini `generateContent` adapter.
//!
//! Protocol facts used here, verified against the published Gemini API
//! reference:
//!
//! - Streaming endpoint:
//!   `POST /v1beta/models/{model}:streamGenerateContent?alt=sse`.
//! - Authentication uses `x-goog-api-key` (existing `XGoogApiKey` mode).
//! - Request roles are `user` and `model`; system text belongs in the
//!   top-level `systemInstruction`, not in `contents`.
//! - With `alt=sse`, each SSE `data` value is one `GenerateContentResponse`
//!   JSON object. It is not Anthropic's named-event stream and not Ollama's
//!   NDJSON stream.
//! - Text arrives in `candidates[].content.parts[].text`; parts flagged
//!   `thought: true` are reasoning, not assistant text.
//! - Finish reasons include `STOP`, `MAX_TOKENS`, `SAFETY`, `RECITATION`,
//!   `BLOCKLIST`, `PROHIBITED_CONTENT`, and `SPII`.
//! - Usage is cumulative in `usageMetadata`.
//!
//! Documentation: <https://ai.google.dev/gemini-api/docs/generate-content/text-generation>
//! and <https://ai.google.dev/gemini-api/docs/generate-content/safety-settings>.

use std::sync::mpsc::SyncSender;

use serde_json::{Value, json};
use tokio::sync::watch;

use super::http::{self, SseDecoder, SseEvent};
use super::{
    ChatEvent, ChatRequest, FinishReason, ProviderError, ProviderSettings, send_event, status_error,
};
use crate::chat::{MAX_ASSISTANT_MESSAGE_BYTES, MessageRole, TokenUsage};

/// Upper bound used for local validation only; the API enforces per-model limits.
pub const MAX_OUTPUT_TOKENS: u32 = 1_000_000;
/// Gemini accepts temperatures from 0 to 2.
pub const MAX_TEMPERATURE: f32 = 2.0;

pub async fn stream(
    settings: &ProviderSettings,
    api_key: Option<&str>,
    request: &ChatRequest,
    sender: &SyncSender<ChatEvent>,
    mut cancel: watch::Receiver<bool>,
) -> Result<(), ProviderError> {
    let body = build_body(settings, request)?;
    let mut url = settings.endpoint()?;
    url.query_pairs_mut().append_pair("alt", "sse");

    let mut state = StreamState::new(sender);
    let mut decoder = SseDecoder::default();
    let result = http::post_stream_with_headers(
        url,
        settings,
        api_key,
        &body,
        "text/event-stream",
        &[],
        &mut cancel,
        |chunk| {
            for event in decoder.push(chunk)? {
                state.handle(event)?;
            }
            Ok(())
        },
    )
    .await;

    match result {
        Ok(()) => {
            for event in decoder.finish()? {
                state.handle(event)?;
            }
            state.emit_finished()
        }
        Err(ProviderError::HttpStatus(status)) => Err(status_error(status)),
        Err(error) => Err(error),
    }
}

/// Builds the JSON body without any protocol fields from other providers.
/// Empty text blocks are skipped; stored order is preserved. Consecutive
/// same-role turns are kept as they are rather than silently merged or given
/// invented turns, so the API's own validation remains visible.
fn build_body(_settings: &ProviderSettings, request: &ChatRequest) -> Result<Value, ProviderError> {
    if !request.tools.is_empty() {
        return Err(ProviderError::InvalidConfig(
            "tool definitions are not implemented for Gemini in this build",
        ));
    }

    let mut contents = Vec::new();
    let mut system_parts = Vec::new();
    if !request.system_prompt.trim().is_empty() {
        system_parts.push(json!({ "text": request.system_prompt }));
    }

    for message in &request.messages {
        if message.role == MessageRole::Tool || !message.tool_calls.is_empty() {
            return Err(ProviderError::InvalidConfig(
                "tool calls and tool results are not implemented for Gemini in this build",
            ));
        }
        if message.role == MessageRole::System {
            if !message.content.trim().is_empty() {
                system_parts.push(json!({ "text": message.content }));
            }
            continue;
        }
        if message.content.trim().is_empty() {
            continue;
        }
        let role = match message.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "model",
            MessageRole::System | MessageRole::Tool => unreachable!(),
        };
        contents.push(json!({
            "role": role,
            "parts": [{ "text": message.content }],
        }));
    }

    if contents.is_empty() {
        return Err(ProviderError::InvalidConfig(
            "Gemini requires at least one non-empty user or assistant message",
        ));
    }

    let mut body = json!({ "contents": contents });
    if !system_parts.is_empty() {
        body["systemInstruction"] = json!({ "parts": system_parts });
    }

    let mut generation = serde_json::Map::new();
    if let Some(temperature) = request.temperature {
        if !temperature.is_finite() || !(0.0..=MAX_TEMPERATURE).contains(&temperature) {
            return Err(ProviderError::InvalidConfig(
                "temperature must be between 0 and 2 for Gemini",
            ));
        }
        generation.insert("temperature".to_owned(), json!(temperature));
    }
    if let Some(max_tokens) = request.max_output_tokens {
        if max_tokens == 0 || max_tokens > MAX_OUTPUT_TOKENS {
            return Err(ProviderError::InvalidConfig(
                "maximum output tokens must be between 1 and 1000000",
            ));
        }
        generation.insert("maxOutputTokens".to_owned(), json!(max_tokens));
    }
    if !generation.is_empty() {
        body["generationConfig"] = Value::Object(generation);
    }

    // The model travels in the path, never in the body.
    Ok(body)
}

struct StreamState<'a> {
    sender: &'a SyncSender<ChatEvent>,
    text_bytes: usize,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    finish: Option<FinishReason>,
    prompt_blocked: bool,
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
            prompt_blocked: false,
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

        if value.get("error").is_some() {
            return Err(ProviderError::RequestFailed);
        }
        if value
            .pointer("/promptFeedback/blockReason")
            .and_then(Value::as_str)
            .is_some()
        {
            self.prompt_blocked = true;
        }

        if let Some(candidates) = value.get("candidates").and_then(Value::as_array) {
            // NEXUS requests a single candidate. Ignore any extra candidates
            // instead of interleaving their text.
            let primary = candidates.iter().find(|candidate| {
                candidate
                    .get("index")
                    .and_then(Value::as_u64)
                    .is_none_or(|index| index == 0)
            });
            if let Some(candidate) = primary {
                if let Some(parts) = candidate
                    .pointer("/content/parts")
                    .and_then(Value::as_array)
                {
                    for part in parts {
                        let Some(text) = part.get("text").and_then(Value::as_str) else {
                            continue;
                        };
                        if part.get("thought").and_then(Value::as_bool) == Some(true) {
                            self.emit_reasoning(text)?;
                        } else {
                            self.emit_text(text)?;
                        }
                    }
                }
                if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
                    self.finish = Some(map_finish_reason(reason));
                }
            }
        }

        if let Some(usage) = value.get("usageMetadata") {
            if let Some(value) = usage.get("promptTokenCount").and_then(Value::as_u64) {
                self.input_tokens = Some(value);
            }
            if let Some(value) = usage.get("candidatesTokenCount").and_then(Value::as_u64) {
                self.output_tokens = Some(value);
            }
            if let Some(value) = usage.get("cachedContentTokenCount").and_then(Value::as_u64) {
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
        let finish = if self.prompt_blocked {
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

/// Maps documented finish reasons onto the small internal outcome model.
fn map_finish_reason(reason: &str) -> FinishReason {
    match reason {
        "STOP" => FinishReason::Completed,
        "MAX_TOKENS" => FinishReason::MaxTokens,
        "SAFETY" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY" => {
            FinishReason::Blocked
        }
        "RECITATION" => FinishReason::Recitation,
        // `FINISH_REASON_UNSPECIFIED`, `OTHER`, `LANGUAGE`, malformed or
        // unexpected tool calls, and any newer value are incomplete, never a
        // silent success.
        _ => FinishReason::Incomplete,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{MessageStatus, StoredMessage};
    use crate::config::{AuthMode, Protocol};
    use crate::provider::ProviderController;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    fn settings(base_url: &str) -> ProviderSettings {
        ProviderSettings {
            profile_id: 1,
            protocol: Protocol::GeminiGenerateContent,
            base_url: base_url.to_owned(),
            auth: AuthMode::XGoogApiKey,
            model: "gemini-2.5-flash".to_owned(),
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

    fn body(request: &ChatRequest) -> Value {
        build_body(
            &settings("https://generativelanguage.googleapis.com/v1beta"),
            request,
        )
        .expect("body")
    }

    fn sse(data: &str) -> SseEvent {
        SseEvent {
            event: None,
            data: data.to_owned(),
        }
    }

    fn serve_once(response: &'static str, bodies: Arc<Mutex<Vec<String>>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).into_owned();
            bodies.lock().expect("lock").push(request);
            stream.write_all(response.as_bytes()).expect("write");
        });
        format!("http://{addr}/v1beta")
    }

    use std::sync::{Arc, Mutex};

    #[test]
    fn request_uses_roles_system_instruction_and_generation_config() {
        let mut req = request(vec![
            message(MessageRole::System, "be brief"),
            message(MessageRole::User, "hello"),
            message(MessageRole::Assistant, "hi"),
            message(MessageRole::User, "again"),
        ]);
        req.system_prompt = "top system".to_owned();
        req.temperature = Some(0.4);
        req.max_output_tokens = Some(123);
        let json = body(&req);
        assert_eq!(json["contents"][0]["role"], "user");
        assert_eq!(json["contents"][0]["parts"][0]["text"], "hello");
        assert_eq!(json["contents"][1]["role"], "model");
        assert_eq!(json["contents"][2]["role"], "user");
        assert_eq!(json["systemInstruction"]["parts"][0]["text"], "top system");
        assert_eq!(json["systemInstruction"]["parts"][1]["text"], "be brief");
        let temperature = json["generationConfig"]["temperature"]
            .as_f64()
            .expect("temperature");
        assert!(
            (temperature - 0.4).abs() < 1e-6,
            "temperature {temperature}"
        );
        assert_eq!(json["generationConfig"]["maxOutputTokens"], 123);
        assert!(json.get("model").is_none());
        assert!(json.get("stream").is_none());
        assert!(json.get("max_tokens").is_none());
        assert!(json.get("system").is_none());
    }

    #[test]
    fn empty_messages_are_skipped_and_order_is_preserved() {
        let json = body(&request(vec![
            message(MessageRole::User, "one"),
            message(MessageRole::User, "   "),
            message(MessageRole::Assistant, "two"),
            message(MessageRole::User, "three"),
        ]));
        let contents = json["contents"].as_array().expect("array");
        assert_eq!(contents.len(), 3);
        assert_eq!(contents[0]["parts"][0]["text"], "one");
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[1]["parts"][0]["text"], "two");
        assert_eq!(contents[2]["parts"][0]["text"], "three");
    }

    #[test]
    fn consecutive_same_role_messages_are_not_merged() {
        let json = body(&request(vec![
            message(MessageRole::User, "one"),
            message(MessageRole::User, "two"),
        ]));
        assert_eq!(json["contents"].as_array().expect("array").len(), 2);
    }

    #[test]
    fn tool_content_is_rejected() {
        let mut req = request(vec![message(MessageRole::Tool, "result")]);
        assert!(matches!(
            build_body(&settings("https://example.com/v1beta"), &req),
            Err(ProviderError::InvalidConfig(_))
        ));
        req.messages = vec![message(MessageRole::User, "hi")];
        req.messages[0].tool_calls = vec![crate::provider::ToolCall {
            id: "1".to_owned(),
            name: "x".to_owned(),
            arguments: "{}".to_owned(),
        }];
        assert!(build_body(&settings("https://example.com/v1beta"), &req).is_err());
    }

    #[test]
    fn temperature_and_output_token_bounds_are_enforced() {
        let mut req = request(vec![message(MessageRole::User, "hi")]);
        req.temperature = Some(2.5);
        assert!(body_result(&req).is_err());
        req.temperature = Some(2.0);
        assert!(body_result(&req).is_ok());
        req.temperature = None;
        req.max_output_tokens = Some(0);
        assert!(body_result(&req).is_err());
        req.max_output_tokens = Some(MAX_OUTPUT_TOKENS + 1);
        assert!(body_result(&req).is_err());
    }

    fn body_result(request: &ChatRequest) -> Result<Value, ProviderError> {
        build_body(&settings("https://example.com/v1beta"), request)
    }

    #[test]
    fn endpoint_builds_streaming_path_and_rejects_bad_models() {
        let settings = settings("https://generativelanguage.googleapis.com/v1beta");
        assert_eq!(
            settings.endpoint().expect("url").as_str(),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:streamGenerateContent"
        );
        let mut with_prefix = settings.clone();
        with_prefix.model = "models/gemini-2.5-flash".to_owned();
        assert_eq!(
            with_prefix.endpoint().expect("url").path(),
            "/v1beta/models/gemini-2.5-flash:streamGenerateContent"
        );
        with_prefix.model = "../evil".to_owned();
        assert!(with_prefix.endpoint().is_err());
    }

    #[test]
    fn sse_events_are_ordered_across_split_chunks_and_multiple_parts() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(16);
        let mut state = StreamState::new(&sender);
        let wire = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hel\"}]}}]}\n\ndata: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"lo \"},{\"text\":\"world\"}]},\"finishReason\":\"STOP\"}]}\n\n";
        let mut decoder = SseDecoder::default();
        for chunk in wire.as_bytes().chunks(7) {
            for event in decoder.push(chunk).expect("decode") {
                state.handle(event).expect("ok");
            }
        }
        for event in decoder.finish().expect("flush") {
            state.handle(event).expect("ok");
        }
        state.emit_finished().expect("finish");
        drop(sender);
        let events: Vec<_> = receiver.try_iter().collect();
        let mut text = String::new();
        for event in &events {
            if let ChatEvent::TextDelta(delta) = event {
                text.push_str(delta);
            }
        }
        assert_eq!(text, "Hello world");
        assert!(matches!(
            events.last(),
            Some(ChatEvent::Finished {
                finish: FinishReason::Completed,
                ..
            })
        ));
    }

    #[test]
    fn thought_parts_stay_out_of_assistant_text() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(16);
        let mut state = StreamState::new(&sender);
        state
            .handle(sse(
                "{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"plan\",\"thought\":true},{\"text\":\"answer\"}]}}]}",
            ))
            .expect("ok");
        drop(sender);
        let events: Vec<_> = receiver.try_iter().collect();
        assert!(matches!(&events[0], ChatEvent::ReasoningDelta(text) if text == "plan"));
        assert!(matches!(&events[1], ChatEvent::TextDelta(text) if text == "answer"));
    }

    #[test]
    fn finish_reasons_map_to_distinct_outcomes() {
        assert_eq!(map_finish_reason("STOP"), FinishReason::Completed);
        assert_eq!(map_finish_reason("MAX_TOKENS"), FinishReason::MaxTokens);
        assert_eq!(map_finish_reason("SAFETY"), FinishReason::Blocked);
        assert_eq!(map_finish_reason("BLOCKLIST"), FinishReason::Blocked);
        assert_eq!(map_finish_reason("RECITATION"), FinishReason::Recitation);
        assert_eq!(map_finish_reason("OTHER"), FinishReason::Incomplete);
        assert_eq!(map_finish_reason("SOMETHING_NEW"), FinishReason::Incomplete);
    }

    #[test]
    fn prompt_block_and_missing_finish_are_not_reported_as_complete() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(16);
        let mut state = StreamState::new(&sender);
        state
            .handle(sse("{\"promptFeedback\":{\"blockReason\":\"SAFETY\"}}"))
            .expect("ok");
        state.emit_finished().expect("finish");
        drop(sender);
        assert!(matches!(
            receiver.try_iter().last(),
            Some(ChatEvent::Finished {
                finish: FinishReason::Blocked,
                ..
            })
        ));
    }

    #[test]
    fn usage_metadata_is_mapped() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(16);
        let mut state = StreamState::new(&sender);
        state
            .handle(sse(
                "{\"candidates\":[],\"usageMetadata\":{\"promptTokenCount\":7,\"candidatesTokenCount\":3,\"cachedContentTokenCount\":2,\"totalTokenCount\":10}}",
            ))
            .expect("ok");
        state.emit_finished().expect("finish");
        drop(sender);
        let events: Vec<_> = receiver.try_iter().collect();
        let Some(ChatEvent::Finished {
            usage: Some(usage), ..
        }) = events.last()
        else {
            panic!("usage missing");
        };
        assert_eq!(usage.input_tokens, 7);
        assert_eq!(usage.output_tokens, 3);
        assert_eq!(usage.cached_input_tokens, 2);
    }

    #[test]
    fn extra_candidates_are_ignored_without_duplicating_text() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(16);
        let mut state = StreamState::new(&sender);
        state
            .handle(sse(
                "{\"candidates\":[{\"index\":0,\"content\":{\"parts\":[{\"text\":\"first\"}]}},{\"index\":1,\"content\":{\"parts\":[{\"text\":\"second\"}]}}]}",
            ))
            .expect("ok");
        drop(sender);
        let text: String = receiver
            .try_iter()
            .filter_map(|event| match event {
                ChatEvent::TextDelta(text) => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(text, "first");
    }

    #[test]
    fn malformed_json_and_error_payloads_fail() {
        let (sender, _receiver) = std::sync::mpsc::sync_channel(8);
        let mut state = StreamState::new(&sender);
        assert!(matches!(
            state.handle(sse("{not json")),
            Err(ProviderError::InvalidResponse)
        ));
        assert!(matches!(
            state.handle(sse("{\"error\":{\"status\":\"INVALID_ARGUMENT\"}}")),
            Err(ProviderError::RequestFailed)
        ));
    }

    #[test]
    fn cancellation_stops_an_open_stream() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
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
            settings(&format!("http://{addr}/v1beta")),
            Some("test-key".to_owned()),
            request(vec![message(MessageRole::User, "hi")]),
        )
        .expect("start");
        std::thread::sleep(Duration::from_millis(100));
        controller.cancel();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut failed = false;
        while Instant::now() < deadline && !failed {
            for event in controller.poll() {
                if matches!(event, ChatEvent::Failed(ProviderError::Cancelled)) {
                    failed = true;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(failed, "expected cancellation");
    }

    #[test]
    fn loopback_stream_extracts_text_and_usage_and_sends_key_header() {
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let response = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n\
data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hi\"}]}}]}\n\n\
data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\" there\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":2,\"candidatesTokenCount\":2}}\n\n";
        let base = serve_once(
            Box::leak(response.to_owned().into_boxed_str()),
            bodies.clone(),
        );
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
        assert!(
            request
                .starts_with("POST /v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse ")
        );
        let lower = request.to_lowercase();
        assert!(lower.contains("x-goog-api-key: secret-key"));
        assert!(!lower.contains("authorization:"));
    }

    #[test]
    fn http_errors_map_to_provider_errors() {
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
            let base = serve_once(Box::leak(response.into_boxed_str()), bodies);
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
    fn incomplete_stream_still_emits_an_incomplete_finish() {
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let response = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"partial\"}]}}]}\n\n";
        let base = serve_once(Box::leak(response.to_owned().into_boxed_str()), bodies);
        let mut controller = ProviderController::start(
            settings(&base),
            Some("key".to_owned()),
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
        assert!(matches!(
            events.last(),
            Some(ChatEvent::Finished {
                finish: FinishReason::Incomplete,
                ..
            })
        ));
    }
}
