use std::time::Instant;

use futures_util::StreamExt;
use reqwest::redirect::Policy;
use tokio::sync::watch;

use super::{
    CONNECT_TIMEOUT, IDLE_TIMEOUT, MAX_RESPONSE_BYTES, MAX_STREAM_LINE_BYTES, ProviderError,
    ProviderSettings, TOTAL_TIMEOUT,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Default)]
pub struct SseDecoder {
    line: Vec<u8>,
    event: Option<String>,
    data: String,
    total_bytes: usize,
}

impl SseDecoder {
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>, ProviderError> {
        self.total_bytes = self.total_bytes.saturating_add(chunk.len());
        if self.total_bytes > MAX_RESPONSE_BYTES {
            return Err(ProviderError::ResponseTooLarge);
        }
        let mut events = Vec::new();
        for &byte in chunk {
            if byte == b'\n' {
                let mut line = std::mem::take(&mut self.line);
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                self.process_line(&line, &mut events)?;
            } else {
                self.line.push(byte);
                if self.line.len() > MAX_STREAM_LINE_BYTES {
                    return Err(ProviderError::ResponseTooLarge);
                }
            }
        }
        Ok(events)
    }

    pub fn finish(&mut self) -> Result<Vec<SseEvent>, ProviderError> {
        let mut events = Vec::new();
        if !self.line.is_empty() {
            let line = std::mem::take(&mut self.line);
            self.process_line(&line, &mut events)?;
        }
        self.emit(&mut events);
        Ok(events)
    }

    fn process_line(
        &mut self,
        line: &[u8],
        events: &mut Vec<SseEvent>,
    ) -> Result<(), ProviderError> {
        if line.is_empty() {
            self.emit(events);
            return Ok(());
        }
        if line.starts_with(b":") {
            return Ok(());
        }
        let line = std::str::from_utf8(line).map_err(|_| ProviderError::InvalidResponse)?;
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "event" => {
                if value.len() > 128 {
                    return Err(ProviderError::InvalidResponse);
                }
                self.event = Some(value.to_owned());
            }
            "data" => {
                if self.data.len().saturating_add(value.len()) > MAX_STREAM_LINE_BYTES {
                    return Err(ProviderError::ResponseTooLarge);
                }
                self.data.push_str(value);
                self.data.push('\n');
            }
            _ => {}
        }
        Ok(())
    }

    fn emit(&mut self, events: &mut Vec<SseEvent>) {
        if self.data.is_empty() {
            self.event = None;
            return;
        }
        let data = std::mem::take(&mut self.data);
        let data = data.trim_end_matches('\n').to_owned();
        events.push(SseEvent {
            event: self.event.take(),
            data,
        });
    }
}

pub async fn post_sse(
    settings: &ProviderSettings,
    api_key: Option<&str>,
    body: &serde_json::Value,
    cancel: &mut watch::Receiver<bool>,
    mut on_event: impl FnMut(SseEvent) -> Result<(), ProviderError>,
) -> Result<(), ProviderError> {
    let url = settings.endpoint()?;
    let headers = super::auth_headers(settings, api_key)?;
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TOTAL_TIMEOUT)
        .redirect(Policy::none())
        .user_agent(format!("NEXUS/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| ProviderError::RequestFailed)?;
    let mut request = client
        .post(url)
        .header("Accept", "text/event-stream")
        .json(body);
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let response = request
        .send()
        .await
        .map_err(|_| ProviderError::RequestFailed)?;
    let status = response.status();
    if !status.is_success() {
        return Err(ProviderError::HttpStatus(status.as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(ProviderError::ResponseTooLarge);
    }

    let mut decoder = SseDecoder::default();
    let mut stream = response.bytes_stream();
    let started = Instant::now();
    loop {
        if *cancel.borrow() {
            return Err(ProviderError::Cancelled);
        }
        if started.elapsed() > TOTAL_TIMEOUT {
            return Err(ProviderError::Timeout);
        }
        let next = tokio::select! {
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    return Err(ProviderError::Cancelled);
                }
                continue;
            }
            item = tokio::time::timeout(IDLE_TIMEOUT, stream.next()) => {
                match item {
                    Err(_) => return Err(ProviderError::Timeout),
                    Ok(None) => break,
                    Ok(Some(Err(_))) => return Err(ProviderError::RequestFailed),
                    Ok(Some(Ok(chunk))) => chunk,
                }
            }
        };
        for event in decoder.push(&next)? {
            on_event(event)?;
        }
    }
    for event in decoder.finish()? {
        on_event(event)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_multiline_sse_events() {
        let mut decoder = SseDecoder::default();
        let events = decoder
            .push(b"event: message\ndata: {\"a\":\ndata: 1}\n\n")
            .unwrap();
        assert_eq!(
            events,
            vec![SseEvent {
                event: Some("message".into()),
                data: "{\"a\":\n1}".into()
            }]
        );
    }

    #[test]
    fn handles_crlf_comments_and_pending_event_on_finish() {
        let mut decoder = SseDecoder::default();
        assert!(decoder.push(b": ping\r\n\r\n").unwrap().is_empty());
        let events = decoder.push(b"data: hello\r\n").unwrap();
        assert!(events.is_empty());
        let events = decoder.finish().unwrap();
        assert_eq!(
            events,
            vec![SseEvent {
                event: None,
                data: "hello".into()
            }]
        );
    }

    #[test]
    fn rejects_oversized_lines() {
        let mut decoder = SseDecoder::default();
        let chunk = vec![b'x'; MAX_STREAM_LINE_BYTES + 1];
        assert_eq!(
            decoder.push(&chunk).unwrap_err(),
            ProviderError::ResponseTooLarge
        );
    }
}
