use std::time::Duration;

use serde_json::{Value, json};
use url::Url;

pub const MAX_MODEL_BYTES: usize = 160;
pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const KEYCHAIN_SERVICE: &str = "com.nexus.app.provider.openai-compatible";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSettings {
    pub base_url: String,
    pub model: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProviderError {
    #[error("provider URL is invalid or not allowed")]
    InvalidUrl,
    #[error("provider model name is empty or too long")]
    InvalidModel,
    #[error("task text is empty or too long")]
    InvalidPrompt,
    #[error("provider request failed; check the endpoint, network, and API key")]
    RequestFailed,
    #[error("provider response was too large")]
    ResponseTooLarge,
    #[error("provider returned an unsupported response")]
    InvalidResponse,
    #[error("could not access the macOS Keychain")]
    Keychain,
    #[error("no API key is saved in the macOS Keychain")]
    MissingKey,
}

impl ProviderSettings {
    pub fn validate(&self) -> Result<Url, ProviderError> {
        let model = self.model.trim();
        if model.is_empty() || model.len() > MAX_MODEL_BYTES {
            return Err(ProviderError::InvalidModel);
        }

        let url = Url::parse(self.base_url.trim()).map_err(|_| ProviderError::InvalidUrl)?;
        let allowed_scheme = url.scheme() == "https";
        let local_http = url.scheme() == "http"
            && url.host_str().is_some_and(|host| {
                host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
            });
        if !(allowed_scheme || local_http)
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(ProviderError::InvalidUrl);
        }
        Ok(url)
    }

    fn chat_completions_url(&self) -> Result<String, ProviderError> {
        let mut url = self.validate()?;
        let path = format!("{}/chat/completions", url.path().trim_end_matches('/'));
        url.set_path(&path);
        Ok(url.to_string())
    }

    fn keychain_account(&self) -> Result<String, ProviderError> {
        Ok(self.validate()?.origin().ascii_serialization())
    }
}

pub fn complete(
    settings: &ProviderSettings,
    api_key: &str,
    prompt: &str,
) -> Result<String, ProviderError> {
    if prompt.trim().is_empty() || prompt.len() > crate::task::MAX_DESCRIPTION_BYTES {
        return Err(ProviderError::InvalidPrompt);
    }
    if api_key.trim().is_empty() {
        return Err(ProviderError::MissingKey);
    }
    let url = settings.chat_completions_url()?;
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .max_redirects(0)
        .build();
    let agent: ureq::Agent = config.into();
    let request = json!({
        "model": settings.model.trim(),
        "messages": [{"role": "user", "content": prompt}],
        "stream": false,
    });
    let mut response = agent
        .post(url)
        .header("Authorization", &format!("Bearer {}", api_key.trim()))
        .send_json(request)
        .map_err(|_| ProviderError::RequestFailed)?;
    if response
        .body()
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(ProviderError::ResponseTooLarge);
    }
    let body: Value = response
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES as u64)
        .read_json()
        .map_err(|error| match error {
            ureq::Error::BodyExceedsLimit(_) => ProviderError::ResponseTooLarge,
            _ => ProviderError::InvalidResponse,
        })?;

    extract_text_response(&body)
}

fn extract_text_response(body: &Value) -> Result<String, ProviderError> {
    body.get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .filter(|content| !content.trim().is_empty())
        .map(str::to_owned)
        .ok_or(ProviderError::InvalidResponse)
}

#[cfg(target_os = "macos")]
pub fn store_api_key(settings: &ProviderSettings, api_key: &str) -> Result<(), ProviderError> {
    if api_key.trim().is_empty() || api_key.len() > 16_384 {
        return Err(ProviderError::MissingKey);
    }
    let account = settings.keychain_account()?;
    let entry =
        keyring::Entry::new(KEYCHAIN_SERVICE, &account).map_err(|_| ProviderError::Keychain)?;
    entry
        .set_password(api_key.trim())
        .map_err(|_| ProviderError::Keychain)
}

#[cfg(target_os = "macos")]
pub fn remove_api_key(settings: &ProviderSettings) -> Result<(), ProviderError> {
    let account = settings.keychain_account()?;
    let entry =
        keyring::Entry::new(KEYCHAIN_SERVICE, &account).map_err(|_| ProviderError::Keychain)?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(ProviderError::Keychain),
    }
}

#[cfg(target_os = "macos")]
pub fn with_saved_api_key<T>(
    settings: &ProviderSettings,
    use_key: impl FnOnce(&str) -> Result<T, ProviderError>,
) -> Result<T, ProviderError> {
    let account = settings.keychain_account()?;
    let entry =
        keyring::Entry::new(KEYCHAIN_SERVICE, &account).map_err(|_| ProviderError::Keychain)?;
    let key = entry.get_password().map_err(|error| match error {
        keyring::Error::NoEntry => ProviderError::MissingKey,
        _ => ProviderError::Keychain,
    })?;
    use_key(&key)
}

#[cfg(not(target_os = "macos"))]
pub fn store_api_key(_settings: &ProviderSettings, _api_key: &str) -> Result<(), ProviderError> {
    Err(ProviderError::Keychain)
}

#[cfg(not(target_os = "macos"))]
pub fn remove_api_key(_settings: &ProviderSettings) -> Result<(), ProviderError> {
    Err(ProviderError::Keychain)
}

#[cfg(not(target_os = "macos"))]
pub fn with_saved_api_key<T>(
    _settings: &ProviderSettings,
    _use_key: impl FnOnce(&str) -> Result<T, ProviderError>,
) -> Result<T, ProviderError> {
    Err(ProviderError::Keychain)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(base_url: &str) -> ProviderSettings {
        ProviderSettings {
            base_url: base_url.to_owned(),
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
            assert_eq!(settings(url).validate(), Err(ProviderError::InvalidUrl));
        }
    }

    #[test]
    fn builds_openai_compatible_endpoint() {
        assert_eq!(
            settings("http://127.0.0.1:3001/v1/")
                .chat_completions_url()
                .unwrap(),
            "http://127.0.0.1:3001/v1/chat/completions"
        );
    }

    #[test]
    fn keychain_identity_is_scoped_to_endpoint_origin() {
        assert_eq!(
            settings("https://api.example.test/v1").keychain_account(),
            settings("https://api.example.test/other").keychain_account()
        );
        assert_ne!(
            settings("https://api.example.test/v1").keychain_account(),
            settings("https://other.example.test/v1").keychain_account()
        );
    }

    #[test]
    fn rejects_invalid_models_and_prompts() {
        let mut invalid_model = settings("https://api.example.test/v1");
        invalid_model.model = " ".to_owned();
        assert_eq!(invalid_model.validate(), Err(ProviderError::InvalidModel));
        assert_eq!(
            complete(&settings("https://api.example.test/v1"), "key", " "),
            Err(ProviderError::InvalidPrompt)
        );
    }

    #[test]
    fn extracts_only_nonempty_text_from_compatible_chat_response() {
        let body = json!({
            "choices": [{"message": {"content": "Hello from the provider."}}]
        });
        assert_eq!(
            extract_text_response(&body).unwrap(),
            "Hello from the provider."
        );
        assert_eq!(
            extract_text_response(&json!({"choices": []})),
            Err(ProviderError::InvalidResponse)
        );
    }
}
