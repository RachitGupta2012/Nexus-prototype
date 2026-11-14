//! Provider connectivity probes and model discovery.
//!
//! These are diagnostics only: a successful probe proves that the endpoint
//! answered a model-listing request. It does not prove that generation works,
//! and nothing here starts an agent or executes a model.
//!
//! Every request uses the configured base URL, the configured authentication
//! mode, a short timeout, and no redirects. Credentials stay in the Keychain and
//! in request headers; they are never logged or persisted.

use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use serde_json::Value;
use url::Url;

use super::anthropic_messages::ANTHROPIC_VERSION;
use super::ollama_chat;
use super::{ProviderError, ProviderSettings, join_path};
use crate::config::Protocol;

pub const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
pub const MAX_DISCOVERED_MODELS: usize = 2_000;
/// Anthropic model listing is cursor-paginated; this bounds total requests.
pub const MAX_MODEL_PAGES: usize = 5;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AuthOutcome {
    /// The protocol needs no credential (for example a local Ollama server).
    #[default]
    NotRequired,
    Accepted,
    Rejected,
    /// The request failed before authentication could be assessed.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionReport {
    pub reachable: bool,
    pub authentication: AuthOutcome,
    pub model_list_parsed: bool,
    pub model_count: Option<usize>,
    pub summary: String,
}

impl ConnectionReport {
    fn unreachable(summary: &str) -> Self {
        Self {
            reachable: false,
            authentication: AuthOutcome::Unknown,
            model_list_parsed: false,
            model_count: None,
            summary: summary.to_owned(),
        }
    }
}

/// URL used to list models for the configured protocol. Only protocols whose
/// listing endpoint is implemented in this build resolve.
pub fn model_list_url(settings: &ProviderSettings) -> Result<Url, ProviderError> {
    let base = settings.validate_endpoint()?;
    match settings.protocol {
        Protocol::OpenAiChat
        | Protocol::OpenAiResponses
        | Protocol::AnthropicMessages
        | Protocol::GeminiGenerateContent => join_path(base, "/models"),
        Protocol::OllamaChat => join_path(base, "/api/tags"),
    }
}

/// Protocol-mandated request headers. Authentication stays with the existing
/// Keychain-backed auth abstraction.
fn protocol_headers(protocol: Protocol) -> &'static [(&'static str, &'static str)] {
    match protocol {
        Protocol::AnthropicMessages => &[("anthropic-version", ANTHROPIC_VERSION)],
        _ => &[],
    }
}

/// Fetches the model list for a supported protocol. Auth and HTTP failures are
/// returned as sanitized errors; the response body is never echoed.
pub async fn fetch_models(
    settings: &ProviderSettings,
    api_key: Option<&str>,
) -> Result<Vec<String>, ProviderError> {
    if settings.protocol == Protocol::AnthropicMessages {
        return fetch_anthropic_models(settings, api_key).await;
    }
    if settings.protocol == Protocol::GeminiGenerateContent {
        return fetch_gemini_models(settings, api_key).await;
    }
    let url = model_list_url(settings)?;
    let (status, body) = http_get(url, settings, api_key).await?;
    match status {
        200 => parse_models(settings.protocol, &body),
        401 => Err(ProviderError::AuthenticationRejected),
        403 => Err(ProviderError::PermissionDenied),
        other => Err(ProviderError::HttpStatus(other)),
    }
}

/// Follows Anthropic's `after_id` cursor until `has_more` is false, bounded by
/// [`MAX_MODEL_PAGES`] and [`MAX_DISCOVERED_MODELS`].
async fn fetch_anthropic_models(
    settings: &ProviderSettings,
    api_key: Option<&str>,
) -> Result<Vec<String>, ProviderError> {
    let mut ids: Vec<String> = Vec::new();
    let mut after_id: Option<String> = None;
    for _ in 0..MAX_MODEL_PAGES {
        let mut url = model_list_url(settings)?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("limit", "1000");
            if let Some(after) = &after_id {
                query.append_pair("after_id", after);
            }
        }
        let (status, body) = http_get(url, settings, api_key).await?;
        let (page_ids, has_more, last_id) = match status {
            200 => parse_anthropic_page(&body)?,
            401 => return Err(ProviderError::AuthenticationRejected),
            403 => return Err(ProviderError::PermissionDenied),
            other => return Err(ProviderError::HttpStatus(other)),
        };
        for id in page_ids {
            if ids.iter().any(|existing| existing == &id) {
                continue;
            }
            ids.push(id);
            if ids.len() >= MAX_DISCOVERED_MODELS {
                return Ok(ids);
            }
        }
        if !has_more {
            return Ok(ids);
        }
        match last_id {
            Some(next) if after_id.as_deref() != Some(next.as_str()) => after_id = Some(next),
            _ => return Ok(ids),
        }
    }
    Ok(ids)
}

/// Follows Gemini's `nextPageToken` until it is absent, bounded by
/// [`MAX_MODEL_PAGES`] and [`MAX_DISCOVERED_MODELS`]. Only models that declare
/// `generateContent` support are returned.
async fn fetch_gemini_models(
    settings: &ProviderSettings,
    api_key: Option<&str>,
) -> Result<Vec<String>, ProviderError> {
    let mut ids: Vec<String> = Vec::new();
    let mut page_token: Option<String> = None;
    for _ in 0..MAX_MODEL_PAGES {
        let mut url = model_list_url(settings)?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("pageSize", "1000");
            if let Some(token) = &page_token {
                query.append_pair("pageToken", token);
            }
        }
        let (status, body) = http_get(url, settings, api_key).await?;
        let (page_ids, next) = match status {
            200 => parse_gemini_page(&body)?,
            401 => return Err(ProviderError::AuthenticationRejected),
            403 => return Err(ProviderError::PermissionDenied),
            other => return Err(ProviderError::HttpStatus(other)),
        };
        for id in page_ids {
            if ids.iter().any(|existing| existing == &id) {
                continue;
            }
            ids.push(id);
            if ids.len() >= MAX_DISCOVERED_MODELS {
                return Ok(ids);
            }
        }
        match next {
            Some(next) if page_token.as_deref() != Some(next.as_str()) => {
                page_token = Some(next);
            }
            _ => return Ok(ids),
        }
    }
    Ok(ids)
}

/// Probes connectivity without generating tokens. The report is deliberately
/// conservative: HTTP status, authentication outcome, and model-list parsing
/// are reported separately from inference, which is never attempted here.
pub async fn test_connection(
    settings: &ProviderSettings,
    api_key: Option<&str>,
) -> Result<ConnectionReport, ProviderError> {
    let url = model_list_url(settings)?;
    let (status, body) = match http_get(url, settings, api_key).await {
        Ok(response) => response,
        Err(ProviderError::Timeout) => {
            return Ok(ConnectionReport::unreachable(
                "The server did not answer within 8 seconds; it may not be running.",
            ));
        }
        Err(ProviderError::RequestFailed) => {
            return Ok(ConnectionReport::unreachable(
                "Could not reach the server (DNS, connection, or TLS failure).",
            ));
        }
        Err(error) => return Err(error),
    };

    let requires_key = settings.requires_key();
    let authentication = if !requires_key {
        AuthOutcome::NotRequired
    } else {
        match status {
            200 => AuthOutcome::Accepted,
            401 | 403 => AuthOutcome::Rejected,
            _ => AuthOutcome::Unknown,
        }
    };

    if status == 200 {
        return Ok(match parse_models(settings.protocol, &body) {
            Ok(models) => ConnectionReport {
                reachable: true,
                authentication,
                model_list_parsed: true,
                model_count: Some(models.len()),
                summary: format!(
                    "Server reachable{}. {} Models were listed; inference was not tested.",
                    match authentication {
                        AuthOutcome::Accepted => " and the API key was accepted",
                        AuthOutcome::NotRequired => "; this protocol needs no API key",
                        _ => "",
                    },
                    match models.len() {
                        1 => "1 model was found.".to_owned(),
                        count => format!("{count} models were found."),
                    }
                ),
            },
            Err(_) => ConnectionReport {
                reachable: true,
                authentication,
                model_list_parsed: false,
                model_count: None,
                summary: "The server responded, but the model list was not in the expected format. Inference was not tested.".to_owned(),
            },
        });
    }

    let summary = match status {
        401 | 403 => format!(
            "Server reachable, but authentication was rejected (HTTP {status}). Check the saved API key."
        ),
        _ => format!(
            "Server reachable, but it answered HTTP {status}. Check the endpoint path and credentials."
        ),
    };
    Ok(ConnectionReport {
        reachable: true,
        authentication,
        model_list_parsed: false,
        model_count: None,
        summary,
    })
}

async fn http_get(
    url: Url,
    settings: &ProviderSettings,
    api_key: Option<&str>,
) -> Result<(u16, Vec<u8>), ProviderError> {
    super::http::get_json_with_headers(
        url,
        settings,
        api_key,
        PROBE_TIMEOUT,
        protocol_headers(settings.protocol),
    )
    .await
}

pub fn parse_models(protocol: Protocol, body: &[u8]) -> Result<Vec<String>, ProviderError> {
    match protocol {
        Protocol::OpenAiChat | Protocol::OpenAiResponses => parse_openai_models(body),
        Protocol::OllamaChat => ollama_chat::parse_tags(body),
        Protocol::AnthropicMessages => Ok(parse_anthropic_page(body)?.0),
        Protocol::GeminiGenerateContent => Ok(parse_gemini_page(body)?.0),
    }
}

/// Parses one Anthropic model page: `{data: [{id}], has_more, last_id}`.
fn parse_anthropic_page(body: &[u8]) -> Result<(Vec<String>, bool, Option<String>), ProviderError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| ProviderError::InvalidResponse)?;
    let models = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or(ProviderError::InvalidResponse)?;
    let mut ids = Vec::new();
    for model in models {
        let id = model.get("id").and_then(Value::as_str).unwrap_or_default();
        let id = id.trim();
        if id.is_empty()
            || id.len() > super::MAX_MODEL_BYTES
            || ids.iter().any(|existing: &String| existing == id)
        {
            continue;
        }
        ids.push(id.to_owned());
    }
    let has_more = value
        .get("has_more")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let last_id = value
        .get("last_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned);
    Ok((ids, has_more, last_id))
}

/// Parses one Gemini model page. Models are included only when
/// `supportedGenerationMethods` contains `generateContent`; ids are the
/// `baseModelId` used in generation paths, falling back to `name` without its
/// `models/` prefix.
fn parse_gemini_page(body: &[u8]) -> Result<(Vec<String>, Option<String>), ProviderError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| ProviderError::InvalidResponse)?;
    let models = value
        .get("models")
        .and_then(Value::as_array)
        .ok_or(ProviderError::InvalidResponse)?;
    let mut ids = Vec::new();
    for model in models {
        let supports_generate = model
            .get("supportedGenerationMethods")
            .and_then(Value::as_array)
            .is_some_and(|methods| {
                methods
                    .iter()
                    .any(|method| method.as_str() == Some("generateContent"))
            });
        if !supports_generate {
            continue;
        }
        let id = model
            .get("baseModelId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .or_else(|| {
                model
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .map(|name| name.strip_prefix("models/").unwrap_or(name))
                    .filter(|id| !id.is_empty())
            })
            .unwrap_or_default();
        if id.len() > super::MAX_MODEL_BYTES || ids.iter().any(|existing: &String| existing == id) {
            continue;
        }
        ids.push(id.to_owned());
        if ids.len() >= MAX_DISCOVERED_MODELS {
            break;
        }
    }
    let next = value
        .get("nextPageToken")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_owned);
    Ok((ids, next))
}

fn parse_openai_models(body: &[u8]) -> Result<Vec<String>, ProviderError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| ProviderError::InvalidResponse)?;
    let models = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or(ProviderError::InvalidResponse)?;
    let mut ids = Vec::new();
    for model in models {
        let id = model.get("id").and_then(Value::as_str).unwrap_or_default();
        let id = id.trim();
        if id.is_empty() || id.len() > super::MAX_MODEL_BYTES {
            continue;
        }
        if ids.iter().any(|existing: &String| existing == id) {
            continue;
        }
        ids.push(id.to_owned());
        if ids.len() >= MAX_DISCOVERED_MODELS {
            break;
        }
    }
    Ok(ids)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeKind {
    Connection,
    Models,
}

#[derive(Debug)]
pub enum ProbeEvent {
    Connection(Result<ConnectionReport, ProviderError>),
    Models(Result<Vec<String>, ProviderError>),
}

/// Runs a probe on a worker thread so the UI never blocks on the network.
pub struct ProbeTask {
    receiver: Receiver<ProbeEvent>,
}

impl ProbeTask {
    pub fn start(
        kind: ProbeKind,
        settings: ProviderSettings,
        api_key: Option<String>,
    ) -> Result<Self, ProviderError> {
        settings.validate_endpoint()?;
        let (sender, receiver) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("nexus-probe".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(_) => {
                        let event = match kind {
                            ProbeKind::Connection => {
                                ProbeEvent::Connection(Err(ProviderError::RequestFailed))
                            }
                            ProbeKind::Models => {
                                ProbeEvent::Models(Err(ProviderError::RequestFailed))
                            }
                        };
                        let _ = sender.send(event);
                        return;
                    }
                };
                let event: Result<ProbeEvent, ProviderError> = runtime.block_on(async {
                    match kind {
                        ProbeKind::Connection => Ok(ProbeEvent::Connection(
                            test_connection(&settings, api_key.as_deref()).await,
                        )),
                        ProbeKind::Models => Ok(ProbeEvent::Models(
                            fetch_models(&settings, api_key.as_deref()).await,
                        )),
                    }
                });
                if let Ok(event) = event {
                    let _ = sender.send(event);
                }
            })
            .map_err(|_| ProviderError::RequestFailed)?;
        Ok(Self { receiver })
    }

    pub fn poll(&mut self) -> Option<ProbeEvent> {
        self.receiver.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AuthMode;

    fn settings(protocol: Protocol, base_url: &str) -> ProviderSettings {
        ProviderSettings {
            profile_id: 1,
            protocol,
            base_url: base_url.to_owned(),
            auth: match protocol {
                Protocol::AnthropicMessages => AuthMode::XApiKey,
                Protocol::GeminiGenerateContent => AuthMode::XGoogApiKey,
                _ => AuthMode::None,
            },
            model: "test-model".to_owned(),
        }
    }

    #[test]
    fn openai_model_url_avoids_duplicate_version_segments() {
        for (base, expected) in [
            (
                "http://127.0.0.1:3001/v1",
                "http://127.0.0.1:3001/v1/models",
            ),
            (
                "http://127.0.0.1:3001/v1/",
                "http://127.0.0.1:3001/v1/models",
            ),
            (
                "http://127.0.0.1:3001/gateway/v1",
                "http://127.0.0.1:3001/gateway/v1/models",
            ),
            (
                "http://127.0.0.1:3001/v1/models",
                "http://127.0.0.1:3001/v1/models",
            ),
            ("http://127.0.0.1:3001", "http://127.0.0.1:3001/models"),
        ] {
            assert_eq!(
                model_list_url(&settings(Protocol::OpenAiChat, base))
                    .unwrap()
                    .as_str(),
                expected,
                "{base}"
            );
        }
    }

    #[test]
    fn ollama_model_url_uses_the_tags_endpoint() {
        assert_eq!(
            model_list_url(&settings(Protocol::OllamaChat, "http://127.0.0.1:11434"))
                .unwrap()
                .as_str(),
            "http://127.0.0.1:11434/api/tags"
        );
        assert_eq!(
            model_list_url(&settings(
                Protocol::OllamaChat,
                "http://127.0.0.1:11434/api/tags"
            ))
            .unwrap()
            .as_str(),
            "http://127.0.0.1:11434/api/tags"
        );
    }

    #[test]
    fn anthropic_model_url_uses_the_versioned_endpoint() {
        assert_eq!(
            model_list_url(&settings(
                Protocol::AnthropicMessages,
                "https://api.anthropic.com/v1"
            ))
            .unwrap()
            .as_str(),
            "https://api.anthropic.com/v1/models"
        );
        assert_eq!(
            model_list_url(&settings(
                Protocol::AnthropicMessages,
                "https://gateway.example.test/anthropic/v1/"
            ))
            .unwrap()
            .as_str(),
            "https://gateway.example.test/anthropic/v1/models"
        );
    }

    #[test]
    fn parses_anthropic_model_pages() {
        let ids = parse_models(
            Protocol::AnthropicMessages,
            br#"{"data":[{"id":"claude-sonnet-4-5"},{"id":"claude-opus-4-1"},{"id":""},{"id":"claude-sonnet-4-5"}],"has_more":true,"last_id":"claude-opus-4-1"}"#,
        )
        .unwrap();
        assert_eq!(ids, vec!["claude-sonnet-4-5", "claude-opus-4-1"]);
        assert_eq!(
            parse_models(Protocol::AnthropicMessages, b"not json").unwrap_err(),
            ProviderError::InvalidResponse
        );
        assert_eq!(
            parse_models(Protocol::AnthropicMessages, br#"{"models":[]}"#).unwrap_err(),
            ProviderError::InvalidResponse
        );
    }

    #[test]
    fn anthropic_model_listing_follows_cursor_pagination() {
        use std::io::{Read, Write};
        let pages = [
            (
                "200 OK",
                r#"{"data":[{"id":"claude-a"}],"has_more":true,"last_id":"claude-a"}"#,
            ),
            (
                "200 OK",
                r#"{"data":[{"id":"claude-b"}],"has_more":false,"last_id":"claude-b"}"#,
            ),
        ];
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for (status, body) in pages {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut buffer = [0u8; 4096];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let _ = sender.send(String::from_utf8_lossy(&buffer[..read]).to_string());
                let response = format!(
                    "HTTP/1.1 {status}
Content-Type: application/json
Content-Length: {}
Connection: close

{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        let settings = ProviderSettings {
            profile_id: 1,
            protocol: Protocol::AnthropicMessages,
            base_url: format!("http://127.0.0.1:{port}/v1"),
            auth: AuthMode::XApiKey,
            model: "claude-sonnet-4-5".to_owned(),
        };
        let ids = block_on(fetch_models(&settings, Some("test-key"))).unwrap();
        assert_eq!(ids, vec!["claude-a", "claude-b"]);
        let first = receiver.recv().unwrap().to_lowercase();
        let second = receiver.recv().unwrap().to_lowercase();
        assert!(first.contains("anthropic-version: 2023-06-01"), "{first}");
        assert!(first.contains("x-api-key: test-key"), "{first}");
        assert!(first.starts_with("get /v1/models?limit=1000 "), "{first}");
        assert!(second.contains("after_id=claude-a"), "{second}");
    }

    #[test]
    fn anthropic_permission_errors_are_not_reported_as_key_rejection() {
        let (port, _request) = serve_once("403 Forbidden", "{}");
        let mut settings = loopback_with_auth(port, AuthMode::XApiKey);
        settings.protocol = Protocol::AnthropicMessages;
        let error = block_on(fetch_models(&settings, Some("test-key"))).unwrap_err();
        assert_eq!(error, ProviderError::PermissionDenied);
    }

    #[test]
    fn responses_model_listing_uses_the_shared_models_endpoint() {
        let mut settings = loopback_with_auth(1, AuthMode::Bearer);
        settings.protocol = Protocol::OpenAiResponses;
        settings.base_url = "https://gateway.example.com/openai/v1".to_owned();
        assert_eq!(
            model_list_url(&settings).expect("url").as_str(),
            "https://gateway.example.com/openai/v1/models"
        );
        assert_eq!(
            parse_models(
                Protocol::OpenAiResponses,
                br#"{"data":[{"id":"gpt-4.1"},{"id":"gpt-4.1"}]}"#
            )
            .expect("ids"),
            vec!["gpt-4.1"]
        );
    }

    #[test]
    fn invalid_urls_fail_before_any_request() {
        assert_eq!(
            model_list_url(&settings(Protocol::OpenAiChat, "ftp://example.test/v1")),
            Err(ProviderError::InvalidBaseUrl)
        );
        assert_eq!(
            model_list_url(&settings(Protocol::OpenAiChat, "http://example.test/v1")),
            Err(ProviderError::InvalidBaseUrl)
        );
        assert_eq!(
            model_list_url(&settings(Protocol::OpenAiChat, "https://example.test/v1")),
            Err(ProviderError::InvalidConfig(
                "remote endpoints require an API key; no-key requests are limited to loopback"
            ))
        );
    }

    #[test]
    fn parses_openai_model_lists() {
        let ids = parse_openai_models(
            br#"{"object":"list","data":[{"id":"gpt-4.1"},{"id":" gpt-4.1 "},{"id":"local-model"},{"missing":"id"},{"id":""}]}"#,
        )
        .unwrap();
        assert_eq!(ids, vec!["gpt-4.1", "local-model"]);
    }

    #[test]
    fn accepts_an_empty_model_list() {
        assert!(parse_openai_models(br#"{"data":[]}"#).unwrap().is_empty());
    }

    #[test]
    fn rejects_malformed_model_lists() {
        assert_eq!(
            parse_openai_models(b"<html>proxy error</html>").unwrap_err(),
            ProviderError::InvalidResponse
        );
        assert_eq!(
            parse_openai_models(br#"{"models":[]}"#).unwrap_err(),
            ProviderError::InvalidResponse
        );
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    /// Serves exactly one HTTP response on loopback and returns its port plus
    /// the request head for assertions. No external network access.
    fn serve_once(
        status: &'static str,
        body: &'static str,
    ) -> (u16, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0u8; 4096];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let _ = sender.send(String::from_utf8_lossy(&buffer[..read]).to_string());
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (port, receiver)
    }

    fn loopback(port: u16) -> ProviderSettings {
        loopback_with_auth(port, AuthMode::None)
    }

    fn loopback_with_auth(port: u16, auth: AuthMode) -> ProviderSettings {
        ProviderSettings {
            profile_id: 1,
            protocol: Protocol::OpenAiChat,
            base_url: format!("http://127.0.0.1:{port}/v1"),
            auth,
            model: "test-model".to_owned(),
        }
    }

    #[test]
    fn connection_probe_reports_reachability_and_models_without_inference() {
        let (port, request) = serve_once(
            "200 OK",
            r#"{"object":"list","data":[{"id":"alpha"},{"id":"beta"}]}"#,
        );
        let report = block_on(test_connection(&loopback(port), None)).unwrap();
        assert!(report.reachable);
        assert!(report.model_list_parsed);
        assert_eq!(report.model_count, Some(2));
        assert_eq!(report.authentication, AuthOutcome::NotRequired);
        assert!(report.summary.contains("inference was not tested"));
        let head = request.recv().unwrap();
        assert!(
            head.starts_with("GET /v1/models HTTP/1.1"),
            "unexpected request: {head}"
        );
    }

    #[test]
    fn fetch_models_returns_ids_from_a_loopback_server() {
        let (port, _request) = serve_once(
            "200 OK",
            r#"{"data":[{"id":"alpha"},{"id":"alpha"},{"id":"beta"}]}"#,
        );
        let ids = block_on(fetch_models(&loopback(port), None)).unwrap();
        assert_eq!(ids, vec!["alpha", "beta"]);
    }

    #[test]
    fn authentication_rejection_is_reported_not_hidden() {
        let (port, _request) = serve_once("401 Unauthorized", r#"{"error":"bad key"}"#);
        let report = block_on(test_connection(
            &loopback_with_auth(port, AuthMode::Bearer),
            Some("not-a-real-key"),
        ))
        .unwrap();
        assert!(report.reachable);
        assert_eq!(report.authentication, AuthOutcome::Rejected);
        assert!(report.summary.contains("HTTP 401"));
        assert!(!format!("{report:?}").contains("not-a-real-key"));
    }

    #[test]
    fn server_errors_are_reported_without_claiming_authentication() {
        let (port, _request) = serve_once("500 Internal Server Error", r#"{"error":"boom"}"#);
        let report = block_on(test_connection(
            &loopback_with_auth(port, AuthMode::Bearer),
            Some("not-a-real-key"),
        ))
        .unwrap();
        assert!(report.reachable);
        assert!(!report.model_list_parsed);
        assert_eq!(report.authentication, AuthOutcome::Unknown);
        assert!(report.summary.contains("HTTP 500"));
    }

    #[test]
    fn malformed_probe_payloads_are_not_reported_as_success() {
        let (port, _request) = serve_once("200 OK", "<html>proxy error</html>");
        let report = block_on(test_connection(&loopback(port), None)).unwrap();
        assert!(report.reachable);
        assert!(!report.model_list_parsed);
        assert_eq!(report.model_count, None);
        assert!(report.summary.contains("not in the expected format"));
    }

    #[test]
    fn unreachable_servers_are_reported_without_panicking() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let report = block_on(test_connection(&loopback(port), None)).unwrap();
        assert!(!report.reachable);
        assert!(!report.model_list_parsed);
        assert_eq!(report.authentication, AuthOutcome::Unknown);
        assert!(report.summary.contains("Could not reach"));
    }

    #[test]
    fn fetch_models_surfaces_auth_rejection_as_an_error() {
        let (port, _request) = serve_once("401 Unauthorized", "{}");
        let error = block_on(fetch_models(&loopback(port), Some("secret"))).unwrap_err();
        assert_eq!(error, ProviderError::AuthenticationRejected);
        assert!(!format!("{error:?}").contains("secret"));
    }

    #[test]
    fn responses_probe_reports_unreachable_servers_without_hanging() {
        let settings = ProviderSettings {
            profile_id: 1,
            protocol: Protocol::OpenAiResponses,
            base_url: "http://127.0.0.1:19999/v1".to_owned(),
            auth: AuthMode::Bearer,
            model: "gpt-4.1".to_owned(),
        };
        let mut task =
            ProbeTask::start(ProbeKind::Connection, settings, Some("key".into())).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if let Some(event) = task.poll() {
                match event {
                    ProbeEvent::Connection(Ok(report)) => {
                        assert!(!report.reachable);
                        assert!(report.summary.contains("Could not reach"));
                        return;
                    }
                    other => panic!("unexpected probe event: {other:?}"),
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "probe never reported a result"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn gemini_loopback(port: u16) -> ProviderSettings {
        ProviderSettings {
            profile_id: 1,
            protocol: Protocol::GeminiGenerateContent,
            base_url: format!("http://127.0.0.1:{port}/v1beta"),
            auth: AuthMode::XGoogApiKey,
            model: "gemini-2.5-flash".to_owned(),
        }
    }

    /// Serves one response per listed body, in order, and returns every request
    /// head. No external network access.
    fn serve_pages(bodies: &'static [&'static str]) -> (u16, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for body in bodies {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut buffer = [0u8; 4096];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let _ = sender.send(String::from_utf8_lossy(&buffer[..read]).to_string());
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (port, receiver)
    }

    #[test]
    fn gemini_probe_lists_only_generate_content_models_and_sends_key() {
        let (port, request) = serve_once(
            "200 OK",
            r#"{"models":[
                {"name":"models/gemini-2.5-flash","baseModelId":"gemini-2.5-flash","supportedGenerationMethods":["generateContent","countTokens"]},
                {"name":"models/text-embedding-004","baseModelId":"text-embedding-004","supportedGenerationMethods":["embedContent"]},
                {"name":"models/mystery","supportedGenerationMethods":[]},
                {"name":"models/gemini-2.5-pro","supportedGenerationMethods":["streamGenerateContent"]}
            ]}"#,
        );
        let report = block_on(test_connection(
            &gemini_loopback(port),
            Some("not-a-real-key"),
        ))
        .unwrap();
        assert!(report.reachable);
        assert!(report.model_list_parsed);
        assert_eq!(report.model_count, Some(1));
        assert_eq!(report.authentication, AuthOutcome::Accepted);
        assert!(report.summary.contains("inference was not tested"));
        let head = request.recv().unwrap();
        assert!(
            head.starts_with("GET /v1beta/models HTTP/1.1"),
            "unexpected request: {head}"
        );
        assert!(
            head.to_lowercase()
                .contains("x-goog-api-key: not-a-real-key")
        );
        assert!(!head.to_lowercase().contains("authorization:"));
    }

    #[test]
    fn gemini_fetch_models_follows_next_page_token() {
        let (port, requests) = serve_pages(&[
            r#"{"models":[{"name":"models/a","baseModelId":"a","supportedGenerationMethods":["generateContent"]}],"nextPageToken":"t1"}"#,
            r#"{"models":[{"name":"models/b","baseModelId":"b","supportedGenerationMethods":["generateContent"]}],"nextPageToken":"t2"}"#,
            r#"{"models":[{"name":"models/a","baseModelId":"a","supportedGenerationMethods":["generateContent"]},{"name":"models/c","baseModelId":"c","supportedGenerationMethods":["generateContent"]}]}"#,
        ]);
        let ids = block_on(fetch_models(&gemini_loopback(port), Some("key"))).unwrap();
        assert_eq!(ids, vec!["a", "b", "c"]);
        let heads: Vec<String> = requests.try_iter().collect();
        assert_eq!(heads.len(), 3);
        assert!(heads[1].contains("pageToken=t1"));
        assert!(heads[2].contains("pageToken=t2"));
    }

    #[test]
    fn gemini_pagination_is_bounded() {
        let (port, requests) = serve_pages(&[
            r#"{"models":[{"name":"models/a","baseModelId":"a","supportedGenerationMethods":["generateContent"]}],"nextPageToken":"t1"}"#,
            r#"{"models":[{"name":"models/b","baseModelId":"b","supportedGenerationMethods":["generateContent"]}],"nextPageToken":"t2"}"#,
            r#"{"models":[{"name":"models/c","baseModelId":"c","supportedGenerationMethods":["generateContent"]}],"nextPageToken":"t3"}"#,
            r#"{"models":[{"name":"models/d","baseModelId":"d","supportedGenerationMethods":["generateContent"]}],"nextPageToken":"t4"}"#,
            r#"{"models":[{"name":"models/e","baseModelId":"e","supportedGenerationMethods":["generateContent"]}],"nextPageToken":"t5"}"#,
            r#"{"models":[{"name":"models/f","baseModelId":"f","supportedGenerationMethods":["generateContent"]}],"nextPageToken":"t6"}"#,
        ]);
        let ids = block_on(fetch_models(&gemini_loopback(port), Some("key"))).unwrap();
        assert_eq!(ids.len(), MAX_MODEL_PAGES);
        let heads: Vec<String> = requests.try_iter().collect();
        assert_eq!(heads.len(), MAX_MODEL_PAGES);
    }

    #[test]
    fn gemini_empty_malformed_and_error_lists_are_handled() {
        let (port, _request) = serve_once("200 OK", r#"{"models":[]}"#);
        let ids = block_on(fetch_models(&gemini_loopback(port), Some("key"))).unwrap();
        assert!(ids.is_empty());

        let (port, _request) = serve_once("200 OK", r#"{"models":"nope"}"#);
        let error = block_on(fetch_models(&gemini_loopback(port), Some("key"))).unwrap_err();
        assert_eq!(error, ProviderError::InvalidResponse);

        for (status, expected) in [
            ("401 Unauthorized", ProviderError::AuthenticationRejected),
            ("403 Forbidden", ProviderError::PermissionDenied),
            ("429 Too Many Requests", ProviderError::HttpStatus(429)),
        ] {
            let (port, _request) = serve_once(status, "{}");
            let error = block_on(fetch_models(&gemini_loopback(port), Some("key"))).unwrap_err();
            assert_eq!(error, expected);
        }
    }

    #[test]
    fn gemini_probe_does_not_echo_the_api_key_or_body() {
        let (port, _request) = serve_once("401 Unauthorized", r#"{"error":"key supersecret"}"#);
        let report =
            block_on(test_connection(&gemini_loopback(port), Some("supersecret"))).unwrap();
        assert_eq!(report.authentication, AuthOutcome::Rejected);
        assert!(!format!("{report:?}").contains("supersecret"));
    }

    #[test]
    fn reports_are_conservative_about_inference() {
        let report = ConnectionReport {
            reachable: true,
            authentication: AuthOutcome::Accepted,
            model_list_parsed: true,
            model_count: Some(2),
            summary: String::new(),
        };
        assert!(!format!("{report:?}").contains("inference"));
    }
}
