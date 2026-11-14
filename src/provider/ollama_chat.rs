//! Native Ollama chat adapter (`POST /api/chat`).
//!
//! Ollama streams newline-delimited JSON rather than SSE, defaults to a
//! different message envelope, and reports token counts as `prompt_eval_count`
//! / `eval_count`. It is implemented separately from the OpenAI-compatible
//! adapter on purpose.

use std::sync::mpsc::SyncSender;

use serde_json::{Value, json};
use tokio::sync::watch;

use super::http;
use super::{
    ChatEvent, ChatRequest, FinishReason, MAX_STREAM_LINE_BYTES, ProviderError, ProviderSettings,
    send_event,
};
use crate::chat::{MAX_ASSISTANT_MESSAGE_BYTES, MessageRole, StoredMessage, TokenUsage};

const MAX_MODEL_LIST: usize = 2_000;

pub async fn stream(
    settings: &ProviderSettings,
    api_key: Option<&str>,
    request: &ChatRequest,
    sender: &SyncSender<ChatEvent>,
    mut cancel: watch::Receiver<bool>,
) -> Result<(), ProviderError> {
    let body = build_body(settings, request)?;
    let url = settings.endpoint()?;
    let mut decoder = NdjsonDecoder::default();
    let mut state = StreamState::new(sender);

    let result = http::post_stream(
        url,
        settings,
        api_key,
        &body,
        "application/x-ndjson",
        &mut cancel,
        |chunk| {
            for value in decoder.push(chunk)? {
                state.handle(value)?;
            }
            Ok(())
        },
    )
    .await;

    if let Err(error) = result {
        return Err(classify_stream_error(error));
    }
    for value in decoder.finish()? {
        state.handle(value)?;
    }
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
    });
    let mut options = serde_json::Map::new();
    if let Some(temperature) = request.temperature {
        if !temperature.is_finite() || !(0.0..=2.0).contains(&temperature) {
            return Err(ProviderError::InvalidConfig(
                "temperature must be between 0 and 2",
            ));
        }
        options.insert("temperature".to_owned(), json!(temperature));
    }
    if let Some(max_output_tokens) = request.max_output_tokens {
        if max_output_tokens == 0 {
            return Err(ProviderError::InvalidConfig(
                "maximum output tokens must be greater than zero",
            ));
        }
        options.insert("num_predict".to_owned(), json!(max_output_tokens));
    }
    if !options.is_empty() {
        body["options"] = Value::Object(options);
    }
    Ok(body)
}

fn push_message(messages: &mut Vec<Value>, message: &StoredMessage) {
    // Ollama has no tool-message role in this build; tool history is not sent.
    if message.role == MessageRole::Tool {
        return;
    }
    messages.push(json!({
        "role": message.role.as_str(),
        "content": message.content,
    }));
}

fn classify_stream_error(error: ProviderError) -> ProviderError {
    match error {
        ProviderError::HttpStatus(404) => ProviderError::ModelNotFound,
        other => other,
    }
}

/// Incremental newline-delimited JSON decoder. Chunk boundaries are irrelevant:
/// complete lines are parsed immediately and a partial line is retained.
#[derive(Default)]
pub struct NdjsonDecoder {
    line: Vec<u8>,
    total_bytes: usize,
}

impl NdjsonDecoder {
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<Value>, ProviderError> {
        let mut values = Vec::new();
        for &byte in chunk {
            if byte == b'\n' {
                let line = std::mem::take(&mut self.line);
                self.parse_line(&line, &mut values)?;
            } else {
                self.line.push(byte);
                if self.line.len() > MAX_STREAM_LINE_BYTES {
                    return Err(ProviderError::ResponseTooLarge);
                }
            }
        }
        Ok(values)
    }

    pub fn finish(&mut self) -> Result<Vec<Value>, ProviderError> {
        let mut values = Vec::new();
        if !self.line.is_empty() {
            let line = std::mem::take(&mut self.line);
            self.parse_line(&line, &mut values)?;
        }
        Ok(values)
    }

    fn parse_line(&mut self, line: &[u8], values: &mut Vec<Value>) -> Result<(), ProviderError> {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        self.total_bytes = self.total_bytes.saturating_add(line.len());
        if self.total_bytes > super::MAX_RESPONSE_BYTES {
            return Err(ProviderError::ResponseTooLarge);
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            return Ok(());
        }
        let value: Value =
            serde_json::from_slice(line).map_err(|_| ProviderError::InvalidResponse)?;
        values.push(value);
        Ok(())
    }
}

struct StreamState<'a> {
    sender: &'a SyncSender<ChatEvent>,
    text_bytes: usize,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    finished: bool,
}

impl<'a> StreamState<'a> {
    fn new(sender: &'a SyncSender<ChatEvent>) -> Self {
        Self {
            sender,
            text_bytes: 0,
            input_tokens: None,
            output_tokens: None,
            finished: false,
        }
    }

    fn handle(&mut self, value: Value) -> Result<(), ProviderError> {
        if value.get("error").is_some() {
            return Err(ProviderError::RequestFailed);
        }
        if let Some(message) = value.get("message") {
            if let Some(content) = message.get("content").and_then(Value::as_str) {
                self.emit_text(content)?;
            }
            if let Some(thinking) = message.get("thinking").and_then(Value::as_str) {
                self.emit_reasoning(thinking)?;
            }
        }
        if value.get("done").and_then(Value::as_bool) == Some(true) {
            self.input_tokens = value.get("prompt_eval_count").and_then(Value::as_u64);
            self.output_tokens = value.get("eval_count").and_then(Value::as_u64);
            self.emit_finished()?;
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

    fn emit_finished(&mut self) -> Result<(), ProviderError> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        let usage = match (self.input_tokens, self.output_tokens) {
            (None, None) => None,
            (input, output) => Some(TokenUsage {
                input_tokens: input.unwrap_or(0),
                output_tokens: output.unwrap_or(0),
                cached_input_tokens: 0,
            }),
        };
        send_event(
            self.sender,
            ChatEvent::Finished {
                tool_calls: Vec::new(),
                usage,
                finish: FinishReason::Completed,
            },
        )
    }
}

/// Parses `GET /api/tags`. Names are mapped to model ids as published by the
/// server; nothing is downloaded or installed.
pub fn parse_tags(body: &[u8]) -> Result<Vec<String>, ProviderError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| ProviderError::InvalidResponse)?;
    let models = value
        .get("models")
        .and_then(Value::as_array)
        .ok_or(ProviderError::InvalidResponse)?;
    let mut names = Vec::new();
    for model in models {
        let name = model
            .get("name")
            .or_else(|| model.get("model"))
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        if name.is_empty() || names.iter().any(|existing: &String| existing == name) {
            continue;
        }
        names.push(name.to_owned());
        if names.len() >= MAX_MODEL_LIST {
            break;
        }
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{MessageStatus, ToolCall};

    fn settings() -> ProviderSettings {
        ProviderSettings {
            profile_id: 1,
            protocol: crate::config::Protocol::OllamaChat,
            base_url: "http://127.0.0.1:11434".to_owned(),
            auth: crate::config::AuthMode::None,
            model: "llama3.2".to_owned(),
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
            system_prompt: "Be brief.".to_owned(),
            messages,
            tools: Vec::new(),
            temperature: None,
            max_output_tokens: None,
        }
    }

    #[test]
    fn builds_native_ollama_body() {
        let body = build_body(
            &settings(),
            &request(vec![
                message(MessageRole::User, "hi"),
                message(MessageRole::Assistant, "hello"),
            ]),
        )
        .unwrap();
        assert_eq!(body["model"], "llama3.2");
        assert_eq!(body["stream"], true);
        assert_eq!(
            body["messages"],
            json!([
                {"role": "system", "content": "Be brief."},
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "hello"},
            ])
        );
        assert!(body.get("options").is_none());
    }

    #[test]
    fn maps_options_and_skips_tool_messages() {
        let mut request = request(vec![
            message(MessageRole::User, "hi"),
            message(MessageRole::Tool, "result"),
        ]);
        request.temperature = Some(0.5);
        request.max_output_tokens = Some(64);
        let body = build_body(&settings(), &request).unwrap();
        assert_eq!(
            body["options"],
            json!({"temperature": 0.5, "num_predict": 64})
        );
        assert_eq!(body["messages"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn rejects_invalid_generation_options() {
        let mut request = request(vec![]);
        request.temperature = Some(f32::NAN);
        assert!(matches!(
            build_body(&settings(), &request),
            Err(ProviderError::InvalidConfig(_))
        ));
        request.temperature = None;
        request.max_output_tokens = Some(0);
        assert!(matches!(
            build_body(&settings(), &request),
            Err(ProviderError::InvalidConfig(_))
        ));
    }

    #[test]
    fn uses_the_native_chat_endpoint() {
        assert_eq!(
            settings().endpoint().unwrap().as_str(),
            "http://127.0.0.1:11434/api/chat"
        );
        let mut prefixed = settings();
        prefixed.base_url = "http://127.0.0.1:11434/ollama/".to_owned();
        assert_eq!(
            prefixed.endpoint().unwrap().as_str(),
            "http://127.0.0.1:11434/ollama/api/chat"
        );
        let mut already = settings();
        already.base_url = "http://127.0.0.1:11434/api/chat".to_owned();
        assert_eq!(
            already.endpoint().unwrap().as_str(),
            "http://127.0.0.1:11434/api/chat"
        );
    }

    #[test]
    fn missing_model_http_status_is_classified() {
        assert_eq!(
            classify_stream_error(ProviderError::HttpStatus(404)),
            ProviderError::ModelNotFound
        );
        assert_eq!(
            classify_stream_error(ProviderError::HttpStatus(500)),
            ProviderError::HttpStatus(500)
        );
    }

    #[test]
    fn decodes_multiple_events_in_one_chunk() {
        let mut decoder = NdjsonDecoder::default();
        let values = decoder
            .push(b"{\"message\":{\"content\":\"a\"}}\n{\"done\":true}\n")
            .unwrap();
        assert_eq!(values.len(), 2);
        assert_eq!(values[0]["message"]["content"], "a");
        assert_eq!(values[1]["done"], true);
    }

    #[test]
    fn decodes_an_event_split_across_chunks() {
        let mut decoder = NdjsonDecoder::default();
        assert!(decoder.push(b"{\"message\":{\"cont").unwrap().is_empty());
        assert!(decoder.push(b"ent\":\"hel").unwrap().is_empty());
        let values = decoder.push(b"lo\"}}\n").unwrap();
        assert_eq!(values.len(), 1);
        assert_eq!(values[0]["message"]["content"], "hello");
    }

    #[test]
    fn finish_emits_a_trailing_unterminated_event() {
        let mut decoder = NdjsonDecoder::default();
        assert!(decoder.push(b"{\"done\":true}").unwrap().is_empty());
        assert_eq!(decoder.finish().unwrap()[0]["done"], true);
    }

    #[test]
    fn ignores_blank_lines_and_crlf() {
        let mut decoder = NdjsonDecoder::default();
        let values = decoder.push(b"\r\n  \n{\"done\":true}\r\n").unwrap();
        assert_eq!(values.len(), 1);
    }

    #[test]
    fn rejects_malformed_json_instead_of_crashing() {
        let mut decoder = NdjsonDecoder::default();
        assert_eq!(
            decoder.push(b"not json\n").unwrap_err(),
            ProviderError::InvalidResponse
        );
    }

    #[test]
    fn rejects_oversized_lines() {
        let mut decoder = NdjsonDecoder::default();
        let chunk = vec![b'x'; MAX_STREAM_LINE_BYTES + 1];
        assert_eq!(
            decoder.push(&chunk).unwrap_err(),
            ProviderError::ResponseTooLarge
        );
    }

    fn collect(values: &[&str]) -> (Vec<ChatEvent>, bool) {
        let (sender, receiver) = std::sync::mpsc::sync_channel(64);
        let mut state = StreamState::new(&sender);
        for value in values {
            state.handle(serde_json::from_str(value).unwrap()).unwrap();
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

    fn delta(event: &ChatEvent) -> Option<&str> {
        match event {
            ChatEvent::TextDelta(text) => Some(text.as_str()),
            _ => None,
        }
    }

    #[test]
    fn streams_text_until_the_done_event_and_reports_usage() {
        let (events, finished) = collect(&[
            r#"{"message":{"role":"assistant","content":"Hel"}}"#,
            r#"{"message":{"role":"assistant","content":"lo"}}"#,
            r#"{"done":true,"prompt_eval_count":7,"eval_count":2}"#,
        ]);
        assert!(finished);
        assert_eq!(delta(&events[0]), Some("Hel"));
        assert_eq!(delta(&events[1]), Some("lo"));
        match events.last().unwrap() {
            ChatEvent::Finished {
                usage, tool_calls, ..
            } => {
                assert!(tool_calls.is_empty());
                assert_eq!(
                    usage,
                    &Some(TokenUsage {
                        input_tokens: 7,
                        output_tokens: 2,
                        cached_input_tokens: 0
                    })
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn incomplete_stream_still_finishes_without_losing_text() {
        let (events, finished) = collect(&[r#"{"message":{"content":"partial"}}"#]);
        assert!(finished);
        assert_eq!(delta(&events[0]), Some("partial"));
        assert!(matches!(
            events.last().unwrap(),
            ChatEvent::Finished { usage: None, .. }
        ));
    }

    #[test]
    fn emits_thinking_as_reasoning() {
        let (events, _) = collect(&[r#"{"message":{"thinking":"plan","content":""}}"#]);
        match &events[0] {
            ChatEvent::ReasoningDelta(text) => assert_eq!(text, "plan"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn error_events_fail_the_stream() {
        let (sender, _receiver) = std::sync::mpsc::sync_channel(8);
        let mut state = StreamState::new(&sender);
        let value: Value = serde_json::from_str(r#"{"error":"model not found"}"#).unwrap();
        assert_eq!(state.handle(value), Err(ProviderError::RequestFailed));
    }

    /// Manual verification against a real local server. Ignored by default so
    /// automated runs never depend on or start Ollama:
    ///
    /// ```sh
    /// NEXUS_OLLAMA_MODEL=llama3.2 cargo test --workspace live_ollama -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "requires a local Ollama server and an installed model"]
    fn live_ollama_streams_a_reply() {
        let base = std::env::var("NEXUS_OLLAMA_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:11434".to_owned());
        let model = std::env::var("NEXUS_OLLAMA_MODEL").unwrap_or_else(|_| "llama3.2".to_owned());
        let settings = ProviderSettings {
            profile_id: 1,
            protocol: crate::config::Protocol::OllamaChat,
            base_url: base,
            auth: crate::config::AuthMode::None,
            model,
        };
        let request = ChatRequest {
            system_prompt: String::new(),
            messages: vec![message(MessageRole::User, "Reply with the single word: ok")],
            tools: Vec::new(),
            temperature: Some(0.0),
            max_output_tokens: Some(16),
        };
        let (sender, receiver) = std::sync::mpsc::sync_channel(64);
        let (_cancel_sender, cancel) = tokio::sync::watch::channel(false);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut text = String::new();
        let mut finished = false;
        runtime
            .block_on(stream(&settings, None, &request, &sender, cancel))
            .expect("local Ollama stream failed");
        while let Ok(event) = receiver.recv_timeout(std::time::Duration::from_secs(30)) {
            match event {
                ChatEvent::TextDelta(delta) => text.push_str(&delta),
                ChatEvent::Finished { .. } => {
                    finished = true;
                    break;
                }
                ChatEvent::Failed(error) => panic!("provider error: {error}"),
                ChatEvent::ReasoningDelta(_) => {}
            }
        }
        assert!(finished, "the stream must emit a final event");
        assert!(!text.trim().is_empty(), "expected generated text");
        println!("Ollama replied: {text}");
    }

    #[test]
    fn parses_tags_and_maps_names() {
        let names = parse_tags(
            br#"{"models":[{"name":"llama3.2:latest"},{"model":"qwen2.5"},{"name":""},{"name":"llama3.2:latest"}]}"#,
        )
        .unwrap();
        assert_eq!(names, vec!["llama3.2:latest", "qwen2.5"]);
    }

    #[test]
    fn parses_an_empty_tag_list() {
        assert!(parse_tags(br#"{"models":[]}"#).unwrap().is_empty());
    }

    #[test]
    fn rejects_malformed_tag_payloads() {
        assert_eq!(
            parse_tags(b"not json").unwrap_err(),
            ProviderError::InvalidResponse
        );
        assert_eq!(
            parse_tags(br#"{"unexpected":true}"#).unwrap_err(),
            ProviderError::InvalidResponse
        );
    }

    #[test]
    fn assistant_tool_calls_are_not_sent_to_ollama() {
        let mut with_calls = message(MessageRole::Assistant, "");
        with_calls.tool_calls = vec![ToolCall {
            id: "call_1".to_owned(),
            name: "lookup".to_owned(),
            arguments: "{}".to_owned(),
        }];
        let body = build_body(&settings(), &request(vec![with_calls])).unwrap();
        assert_eq!(body["messages"][1]["content"], "");
    }
}
