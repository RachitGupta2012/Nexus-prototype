//! Direct model-provider clients. Every protocol is an explicit adapter over
//! documented HTTP APIs. No agent process is launched from this layer.

mod http;
pub mod openai_chat;

use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;

use serde_json::Value;
use tokio::sync::watch;
use url::Url;

use crate::chat::{StoredMessage, TokenUsage, ToolCall};
use crate::config::{AuthMode, Protocol};
use crate::credentials;

pub const MAX_MODEL_BYTES: usize = 160;
pub const MAX_BASE_URL_BYTES: usize = 2_048;
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_STREAM_LINE_BYTES: usize = 1024 * 1024;
pub const MAX_TOOL_DEFINITION_BYTES: usize = 256 * 1024;
pub const MAX_TOOL_DESCRIPTION_BYTES: usize = 8 * 1024;
pub const MAX_TOOL_CALLS: usize = 32;

pub const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
pub const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
pub const TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderSettings {
    pub profile_id: u64,
    pub protocol: Protocol,
    pub base_url: String,
    pub auth: AuthMode,
    pub model: String,
}

impl ProviderSettings {
    pub fn validate(&self) -> Result<Url, ProviderError> {
        let model = self.model.trim();
        if model.is_empty() || model.len() > MAX_MODEL_BYTES {
            return Err(ProviderError::InvalidModel);
        }
        if self.base_url.len() > MAX_BASE_URL_BYTES {
            return Err(ProviderError::InvalidBaseUrl);
        }
        let url = Url::parse(self.base_url.trim()).map_err(|_| ProviderError::InvalidBaseUrl)?;
        if url.username() != ""
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(ProviderError::InvalidBaseUrl);
        }
        let loopback = is_loopback(&url);
        let scheme_ok = url.scheme() == "https" || (url.scheme() == "http" && loopback);
        if !scheme_ok || url.host_str().is_none() {
            return Err(ProviderError::InvalidBaseUrl);
        }
        if !self.protocol.allowed_auth().contains(&self.auth) {
            return Err(ProviderError::InvalidConfig(
                "the selected authentication mode does not match this protocol",
            ));
        }
        if self.auth == AuthMode::None && !loopback {
            return Err(ProviderError::InvalidConfig(
                "remote endpoints require an API key; no-key requests are limited to loopback",
            ));
        }
        Ok(url)
    }

    pub fn endpoint(&self) -> Result<Url, ProviderError> {
        let base = self.validate()?;
        let path = match self.protocol {
            Protocol::OpenAiChat => "/chat/completions",
            Protocol::OpenAiResponses => "/responses",
            Protocol::AnthropicMessages => "/messages",
            Protocol::GeminiGenerateContent => return Ok(base),
            Protocol::OllamaChat => "/api/chat",
        };
        join_path(base, path)
    }

    pub fn account(&self) -> Result<String, ProviderError> {
        let origin = self.validate()?.origin().ascii_serialization();
        Ok(credentials::provider_account(self.profile_id, &origin))
    }

    pub fn requires_key(&self) -> bool {
        self.auth != AuthMode::None
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Clone, Debug)]
pub struct ChatRequest {
    pub system_prompt: String,
    pub messages: Vec<StoredMessage>,
    pub tools: Vec<ToolDefinition>,
    pub temperature: Option<f32>,
    pub max_output_tokens: Option<u32>,
}

#[derive(Clone, Debug)]
pub enum ChatEvent {
    TextDelta(String),
    ReasoningDelta(String),
    Finished {
        tool_calls: Vec<ToolCall>,
        usage: Option<TokenUsage>,
    },
    Failed(ProviderError),
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProviderError {
    #[error("provider base URL is invalid or not allowed")]
    InvalidBaseUrl,
    #[error("model name is empty or too long")]
    InvalidModel,
    #[error("provider configuration is invalid: {0}")]
    InvalidConfig(&'static str),
    #[error("no API key is saved for this provider")]
    MissingKey,
    #[error("this provider protocol is not implemented in this build")]
    UnsupportedProtocol,
    #[error("provider request failed; check the endpoint, network, and credentials")]
    RequestFailed,
    #[error("provider returned HTTP {0}")]
    HttpStatus(u16),
    #[error("provider response was too large")]
    ResponseTooLarge,
    #[error("provider returned an unsupported response")]
    InvalidResponse,
    #[error("provider request timed out")]
    Timeout,
    #[error("provider request was cancelled")]
    Cancelled,
}

pub struct ProviderController {
    receiver: Receiver<ChatEvent>,
    cancel: watch::Sender<bool>,
    finished: bool,
}

impl ProviderController {
    pub fn start(
        settings: ProviderSettings,
        api_key: Option<String>,
        request: ChatRequest,
    ) -> Result<Self, ProviderError> {
        settings.validate()?;
        if settings.requires_key() && api_key.as_deref().map(str::trim).is_none_or(str::is_empty) {
            return Err(ProviderError::MissingKey);
        }
        if settings.protocol != Protocol::OpenAiChat {
            return Err(ProviderError::UnsupportedProtocol);
        }

        let (sender, receiver) = mpsc::sync_channel(1_024);
        let (cancel_sender, cancel_receiver) = watch::channel(false);
        thread::Builder::new()
            .name("nexus-provider".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(_) => {
                        let _ = sender.send(ChatEvent::Failed(ProviderError::RequestFailed));
                        return;
                    }
                };
                let result = runtime.block_on(openai_chat::stream(
                    &settings,
                    api_key.as_deref(),
                    &request,
                    &sender,
                    cancel_receiver,
                ));
                if let Err(error) = result {
                    let _ = sender.send(ChatEvent::Failed(error));
                }
            })
            .map_err(|_| ProviderError::RequestFailed)?;

        Ok(Self {
            receiver,
            cancel: cancel_sender,
            finished: false,
        })
    }

    pub fn poll(&mut self) -> Vec<ChatEvent> {
        let mut events = Vec::new();
        loop {
            match self.receiver.try_recv() {
                Ok(event) => events.push(event),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.finished = true;
                    break;
                }
            }
        }
        events
    }

    pub fn cancel(&self) {
        let _ = self.cancel.send(true);
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }
}

pub(crate) fn join_path(mut base: Url, suffix: &str) -> Result<Url, ProviderError> {
    let base_path = base.path().trim_end_matches('/');
    let suffix = suffix.trim_end_matches('/');
    let path = if base_path.ends_with(suffix) {
        base_path.to_owned()
    } else {
        format!("{base_path}{suffix}")
    };
    base.set_path(&path);
    Ok(base)
}

pub(crate) fn is_loopback(url: &Url) -> bool {
    url.host_str().is_some_and(|host| {
        let host = host.trim_start_matches('[').trim_end_matches(']');
        host.eq_ignore_ascii_case("localhost")
            || host == "127.0.0.1"
            || host == "::1"
            || host.starts_with("127.")
    })
}

pub(crate) fn send_event(
    sender: &SyncSender<ChatEvent>,
    event: ChatEvent,
) -> Result<(), ProviderError> {
    sender.send(event).map_err(|_| ProviderError::Cancelled)
}

pub(crate) fn auth_headers(
    settings: &ProviderSettings,
    api_key: Option<&str>,
) -> Result<Vec<(&'static str, String)>, ProviderError> {
    let key = api_key.unwrap_or("").trim();
    if settings.requires_key() && key.is_empty() {
        return Err(ProviderError::MissingKey);
    }
    Ok(match settings.auth {
        AuthMode::None => Vec::new(),
        AuthMode::Bearer => vec![("Authorization", format!("Bearer {key}"))],
        AuthMode::XApiKey => vec![("x-api-key", key.to_owned())],
        AuthMode::XGoogApiKey => vec![("x-goog-api-key", key.to_owned())],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(base_url: &str) -> ProviderSettings {
        ProviderSettings {
            profile_id: 1,
            protocol: Protocol::OpenAiChat,
            base_url: base_url.to_owned(),
            auth: AuthMode::Bearer,
            model: "auto".to_owned(),
        }
    }

    #[test]
    fn accepts_https_and_loopback_http_only() {
        assert!(settings("https://api.example.test/v1").validate().is_ok());
        assert!(settings("http://127.0.0.1:3001/v1").validate().is_ok());
        assert!(settings("http://localhost:3001/v1").validate().is_ok());
        assert!(settings("http://api.example.test/v1").validate().is_err());
    }

    #[test]
    fn rejects_url_credentials_query_and_fragment() {
        for url in [
            "https://user:secret@example.test/v1",
            "https://api.example.test/v1?key=secret",
            "https://api.example.test/v1#fragment",
        ] {
            assert_eq!(settings(url).validate(), Err(ProviderError::InvalidBaseUrl));
        }
    }

    #[test]
    fn no_key_is_limited_to_loopback() {
        let mut local = settings("http://127.0.0.1:3001/v1");
        local.auth = AuthMode::None;
        assert!(local.validate().is_ok());
        let mut remote = settings("https://api.example.test/v1");
        remote.auth = AuthMode::None;
        assert!(matches!(
            remote.validate(),
            Err(ProviderError::InvalidConfig(_))
        ));
    }

    #[test]
    fn builds_openai_compatible_endpoint_without_double_slashes() {
        assert_eq!(
            settings("http://127.0.0.1:3001/v1/")
                .endpoint()
                .unwrap()
                .as_str(),
            "http://127.0.0.1:3001/v1/chat/completions"
        );
        assert_eq!(
            settings("http://127.0.0.1:3001/v1/chat/completions")
                .endpoint()
                .unwrap()
                .as_str(),
            "http://127.0.0.1:3001/v1/chat/completions"
        );
    }

    #[test]
    fn auth_headers_follow_the_selected_mode() {
        let mut bearer = settings("https://api.example.test/v1");
        assert_eq!(
            auth_headers(&bearer, Some("secret")).unwrap(),
            vec![("Authorization", "Bearer secret".to_owned())]
        );
        bearer.auth = AuthMode::None;
        assert!(auth_headers(&bearer, None).unwrap().is_empty());
        let mut anthropic = settings("https://api.anthropic.com/v1");
        anthropic.auth = AuthMode::XApiKey;
        assert_eq!(
            auth_headers(&anthropic, Some("secret")).unwrap(),
            vec![("x-api-key", "secret".to_owned())]
        );
    }

    #[test]
    fn missing_key_is_reported_before_network_access() {
        let error = ProviderController::start(
            settings("https://api.example.test/v1"),
            None,
            ChatRequest {
                system_prompt: String::new(),
                messages: Vec::new(),
                tools: Vec::new(),
                temperature: None,
                max_output_tokens: None,
            },
        )
        .err()
        .unwrap();
        assert_eq!(error, ProviderError::MissingKey);
    }
}
