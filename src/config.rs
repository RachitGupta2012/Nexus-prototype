use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::AppError;

pub const CONFIG_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Dark,
    Light,
}

impl Theme {
    pub const ALL: [Self; 3] = [Self::System, Self::Dark, Self::Light];

    pub const fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Dark => "Dark",
            Self::Light => "Light",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    OpenAiChat,
    OpenAiResponses,
    AnthropicMessages,
    GeminiGenerateContent,
    OllamaChat,
}

impl Protocol {
    pub const ALL: [Self; 5] = [
        Self::OpenAiChat,
        Self::OpenAiResponses,
        Self::AnthropicMessages,
        Self::GeminiGenerateContent,
        Self::OllamaChat,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::OpenAiChat => "OpenAI Chat Completions",
            Self::OpenAiResponses => "OpenAI Responses",
            Self::AnthropicMessages => "Anthropic Messages",
            Self::GeminiGenerateContent => "Google Gemini",
            Self::OllamaChat => "Ollama native",
        }
    }

    pub const fn short_label(self) -> &'static str {
        match self {
            Self::OpenAiChat => "OpenAI Chat",
            Self::OpenAiResponses => "OpenAI Responses",
            Self::AnthropicMessages => "Anthropic",
            Self::GeminiGenerateContent => "Gemini",
            Self::OllamaChat => "Ollama",
        }
    }

    pub const fn default_base_url(self) -> &'static str {
        match self {
            Self::OpenAiChat | Self::OpenAiResponses => "https://api.openai.com/v1",
            Self::AnthropicMessages => "https://api.anthropic.com/v1",
            Self::GeminiGenerateContent => "https://generativelanguage.googleapis.com/v1beta",
            Self::OllamaChat => "http://127.0.0.1:11434",
        }
    }

    pub const fn default_auth(self) -> AuthMode {
        match self {
            Self::OpenAiChat | Self::OpenAiResponses => AuthMode::Bearer,
            Self::AnthropicMessages => AuthMode::XApiKey,
            Self::GeminiGenerateContent => AuthMode::XGoogApiKey,
            Self::OllamaChat => AuthMode::None,
        }
    }

    pub const fn allowed_auth(self) -> &'static [AuthMode] {
        match self {
            Self::OpenAiChat | Self::OpenAiResponses => &[AuthMode::Bearer, AuthMode::None],
            Self::AnthropicMessages => &[AuthMode::XApiKey],
            Self::GeminiGenerateContent => &[AuthMode::XGoogApiKey],
            Self::OllamaChat => &[AuthMode::None],
        }
    }

    pub const fn supports_tools(self) -> bool {
        // Anthropic, Gemini, and OpenAI Responses tool use are not implemented
        // in this build; tool content is rejected rather than reshaped.
        !matches!(
            self,
            Self::AnthropicMessages | Self::GeminiGenerateContent | Self::OpenAiResponses
        )
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    None,
    #[default]
    Bearer,
    XApiKey,
    XGoogApiKey,
}

impl AuthMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "No API key",
            Self::Bearer => "Bearer token",
            Self::XApiKey => "x-api-key",
            Self::XGoogApiKey => "x-goog-api-key",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    #[serde(default)]
    pub cached_input_per_mtok: Option<f64>,
    #[serde(default = "default_currency")]
    pub currency: String,
    #[serde(default)]
    pub reviewed_at_unix_secs: Option<u64>,
}

fn default_currency() -> String {
    "USD".to_owned()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelConfig {
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub supports_tools: Option<bool>,
    #[serde(default)]
    pub pricing: Option<ModelPricing>,
}

impl ModelConfig {
    pub fn new(id: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            display_name: id.clone(),
            id,
            context_window: None,
            supports_tools: None,
            pricing: None,
        }
    }

    pub fn label(&self) -> &str {
        if self.display_name.trim().is_empty() {
            &self.id
        } else {
            &self.display_name
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderProfile {
    pub id: u64,
    pub name: String,
    #[serde(default = "default_protocol")]
    pub protocol: Protocol,
    pub base_url: String,
    #[serde(default)]
    pub auth: AuthMode,
    #[serde(default, alias = "model")]
    pub default_model: String,
    #[serde(default)]
    pub models: Vec<ModelConfig>,
}

fn default_protocol() -> Protocol {
    Protocol::OpenAiChat
}

impl ProviderProfile {
    pub fn new(
        id: u64,
        name: impl Into<String>,
        protocol: Protocol,
        base_url: impl Into<String>,
        auth: AuthMode,
        model: impl Into<String>,
    ) -> Self {
        let model = model.into();
        Self {
            id,
            name: name.into(),
            protocol,
            base_url: base_url.into(),
            auth,
            default_model: model.clone(),
            models: vec![ModelConfig::new(model)],
        }
    }

    pub fn model(&self, id: &str) -> Option<&ModelConfig> {
        self.models.iter().find(|model| model.id == id)
    }

    pub fn ensure_model(&mut self, id: &str) {
        if id.trim().is_empty() {
            return;
        }
        if !self.models.iter().any(|model| model.id == id) {
            self.models.push(ModelConfig::new(id));
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    #[default]
    Stdio,
    StreamableHttp,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub transport: McpTransport,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env_allowlist: Vec<String>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub bearer_auth: bool,
    #[serde(default)]
    pub enabled_tools: Vec<String>,
    /// Disabled servers keep their settings but cannot be connected.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_config_version")]
    pub version: u32,
    #[serde(default)]
    pub default_provider_id: Option<u64>,
    #[serde(default)]
    pub providers: Vec<ProviderProfile>,
    #[serde(default)]
    pub provider_sequence: u64,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    #[serde(default)]
    pub mcp_sequence: u64,
    #[serde(default = "default_true")]
    pub history_enabled: bool,
    #[serde(
        default = "default_retention_days",
        deserialize_with = "deserialize_retention_days"
    )]
    pub history_retention_days: u32,
    #[serde(default = "default_true")]
    pub save_tool_outputs: bool,
    #[serde(default)]
    pub first_run_callout_dismissed: bool,
    #[serde(default)]
    pub theme: Theme,
    // Phase 3 compatibility. These keys are read once and never written back.
    #[serde(default, rename = "provider_base_url", skip_serializing)]
    pub legacy_provider_base_url: Option<String>,
    #[serde(default, rename = "provider_model", skip_serializing)]
    pub legacy_provider_model: Option<String>,
    #[serde(default, rename = "welcome_message", skip_serializing)]
    pub legacy_welcome_message: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_config_version() -> u32 {
    CONFIG_VERSION
}

fn default_retention_days() -> u32 {
    365
}

/// Accept legacy or hand-edited values and clamp them instead of failing startup.
fn deserialize_retention_days<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = i64::deserialize(deserializer)?;
    Ok(value.clamp(1, 3_650) as u32)
}

impl Default for Config {
    fn default() -> Self {
        let profile = ProviderProfile::new(
            1,
            "Local gateway",
            Protocol::OpenAiChat,
            "http://127.0.0.1:3001/v1",
            AuthMode::None,
            "auto",
        );
        Self {
            version: CONFIG_VERSION,
            default_provider_id: Some(profile.id),
            provider_sequence: profile.id,
            providers: vec![profile],
            mcp_servers: Vec::new(),
            mcp_sequence: 0,
            history_enabled: true,
            history_retention_days: default_retention_days(),
            save_tool_outputs: true,
            first_run_callout_dismissed: false,
            theme: Theme::System,
            legacy_provider_base_url: None,
            legacy_provider_model: None,
            legacy_welcome_message: None,
        }
    }
}

impl Config {
    pub fn load_or_default(path: &Path) -> Result<Self, AppError> {
        match fs::read_to_string(path) {
            Ok(contents) => {
                let mut config: Config =
                    toml::from_str(&contents).map_err(|source| AppError::ConfigParse {
                        path: path.to_owned(),
                        source,
                    })?;
                config.normalize();
                Ok(config)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(AppError::ConfigRead {
                path: path.to_owned(),
                source,
            }),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), AppError> {
        let contents = toml::to_string_pretty(self)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| AppError::ConfigWrite {
                path: parent.to_owned(),
                source,
            })?;
        }
        let temporary = path.with_extension("toml.tmp");
        fs::write(&temporary, contents).map_err(|source| AppError::ConfigWrite {
            path: temporary.clone(),
            source,
        })?;
        fs::rename(&temporary, path).map_err(|source| AppError::ConfigWrite {
            path: path.to_owned(),
            source,
        })
    }

    /// Repair imported data and fill defaults without discarding user entries.
    pub fn normalize(&mut self) {
        self.version = CONFIG_VERSION;
        self.history_retention_days = self.history_retention_days.clamp(1, 3_650);

        if self.providers.is_empty()
            && let Some(base_url) = self.legacy_provider_base_url.take()
        {
            let model = self
                .legacy_provider_model
                .take()
                .filter(|model| !model.trim().is_empty())
                .unwrap_or_else(|| "auto".to_owned());
            self.providers.push(ProviderProfile::new(
                1,
                "Imported provider",
                Protocol::OpenAiChat,
                base_url,
                AuthMode::Bearer,
                model,
            ));
        }
        self.legacy_provider_base_url = None;
        self.legacy_provider_model = None;
        self.legacy_welcome_message = None;

        if self.providers.is_empty() {
            let profile = ProviderProfile::new(
                1,
                "Local gateway",
                Protocol::OpenAiChat,
                "http://127.0.0.1:3001/v1",
                AuthMode::None,
                "auto",
            );
            self.providers.push(profile);
        }

        let mut seen = HashSet::new();
        let mut next_id = self
            .providers
            .iter()
            .map(|provider| provider.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        for provider in &mut self.providers {
            while !seen.insert(provider.id) {
                provider.id = next_id;
                next_id = next_id.saturating_add(1);
            }
            if provider.name.trim().is_empty() {
                provider.name = format!("Provider {}", provider.id);
            }
            provider.name = provider.name.trim().to_owned();
            provider.base_url = provider.base_url.trim().to_owned();
            if !provider.protocol.allowed_auth().contains(&provider.auth) {
                provider.auth = provider.protocol.default_auth();
            }
            let mut seen_models = HashSet::new();
            provider.models.retain(|model| {
                let id = model.id.trim();
                !id.is_empty() && seen_models.insert(id.to_owned())
            });
            for model in &mut provider.models {
                model.id = model.id.trim().to_owned();
                if model.display_name.trim().is_empty() {
                    model.display_name = model.id.clone();
                } else {
                    model.display_name = model.display_name.trim().to_owned();
                }
                if let Some(pricing) = &mut model.pricing {
                    pricing.currency = pricing.currency.trim().to_owned();
                    if pricing.currency.is_empty() {
                        pricing.currency = default_currency();
                    }
                }
            }
            let default_model = provider.default_model.trim().to_owned();
            if default_model.is_empty() {
                provider.default_model = provider
                    .models
                    .first()
                    .map(|model| model.id.clone())
                    .unwrap_or_else(|| "auto".to_owned());
            } else {
                provider.default_model = default_model;
            }
            provider.ensure_model(&provider.default_model.clone());
        }
        self.provider_sequence = self.provider_sequence.max(
            self.providers
                .iter()
                .map(|provider| provider.id)
                .max()
                .unwrap_or(0),
        );

        if self
            .default_provider_id
            .is_none_or(|id| !self.providers.iter().any(|provider| provider.id == id))
        {
            self.default_provider_id = self.providers.first().map(|provider| provider.id);
        }

        let mut seen_mcp = HashSet::new();
        let mut next_mcp_id = self
            .mcp_servers
            .iter()
            .map(|server| server.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        for server in &mut self.mcp_servers {
            while !seen_mcp.insert(server.id) {
                server.id = next_mcp_id;
                next_mcp_id = next_mcp_id.saturating_add(1);
            }
            if server.name.trim().is_empty() {
                server.name = format!("MCP server {}", server.id);
            }
            server.name = server.name.trim().to_owned();
            server.command = server.command.trim().to_owned();
            server.url = server.url.trim().to_owned();
            server.cwd = server
                .cwd
                .take()
                .map(|cwd| cwd.trim().to_owned())
                .filter(|cwd| !cwd.is_empty());
            server.env_allowlist.retain(|name| !name.trim().is_empty());
            for name in &mut server.env_allowlist {
                *name = name.trim().to_owned();
            }
            server.enabled_tools.retain(|name| !name.trim().is_empty());
            for name in &mut server.enabled_tools {
                *name = name.trim().to_owned();
            }
        }
        self.mcp_sequence = self.mcp_sequence.max(
            self.mcp_servers
                .iter()
                .map(|server| server.id)
                .max()
                .unwrap_or(0),
        );
    }

    pub fn provider(&self, id: u64) -> Option<&ProviderProfile> {
        self.providers.iter().find(|provider| provider.id == id)
    }

    pub fn provider_mut(&mut self, id: u64) -> Option<&mut ProviderProfile> {
        self.providers.iter_mut().find(|provider| provider.id == id)
    }

    pub fn default_provider(&self) -> Option<&ProviderProfile> {
        self.default_provider_id
            .and_then(|id| self.provider(id))
            .or_else(|| self.providers.first())
    }

    pub fn allocate_mcp_id(&mut self) -> u64 {
        self.mcp_sequence = self
            .mcp_sequence
            .max(
                self.mcp_servers
                    .iter()
                    .map(|server| server.id)
                    .max()
                    .unwrap_or(0),
            )
            .saturating_add(1);
        self.mcp_sequence
    }

    pub fn allocate_provider_id(&mut self) -> u64 {
        self.provider_sequence = self
            .provider_sequence
            .max(
                self.providers
                    .iter()
                    .map(|provider| provider.id)
                    .max()
                    .unwrap_or(0),
            )
            .saturating_add(1);
        self.provider_sequence
    }

    pub fn remove_provider(&mut self, id: u64) -> bool {
        if self.providers.len() <= 1 {
            return false;
        }
        let Some(index) = self.providers.iter().position(|provider| provider.id == id) else {
            return false;
        };
        self.providers.remove(index);
        if self.default_provider_id == Some(id) {
            self.default_provider_id = self.providers.first().map(|provider| provider.id);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_include_a_local_gateway_provider() {
        let config = Config::default();
        assert_eq!(config.providers.len(), 1);
        assert_eq!(config.default_provider_id, Some(1));
        assert!(config.history_enabled);
        assert_eq!(config.providers[0].auth, AuthMode::None);
    }

    #[test]
    fn parses_a_toml_override_and_defaults_missing_fields() {
        let mut config: Config = toml::from_str("history_enabled = false").unwrap();
        config.normalize();

        assert!(!config.history_enabled);
        assert_eq!(config.version, CONFIG_VERSION);
        assert_eq!(config.providers.len(), 1);
        assert_eq!(config.providers[0].default_model, "auto");
    }

    #[test]
    fn imports_the_phase_three_single_provider_fields() {
        let mut config: Config = toml::from_str(
            "provider_base_url = 'https://legacy.example/v1'\nprovider_model = 'legacy-model'",
        )
        .unwrap();
        config.normalize();

        let provider = config.default_provider().unwrap();
        assert_eq!(provider.base_url, "https://legacy.example/v1");
        assert_eq!(provider.default_model, "legacy-model");
        assert_eq!(provider.protocol, Protocol::OpenAiChat);
        assert_eq!(provider.auth, AuthMode::Bearer);
    }

    #[test]
    fn imports_phase_four_providers_without_protocol_or_display_names() {
        let mut config: Config = toml::from_str(
            "welcome_message = 'hi'\nactive_provider_id = 1\nprovider_sequence = 2\n\
             [[providers]]\nid = 1\nname = 'Local gateway'\nbase_url = 'http://127.0.0.1:3001/v1'\nmodel = 'auto'\n",
        )
        .unwrap();
        config.normalize();

        let provider = config.default_provider().unwrap();
        assert_eq!(provider.id, 1);
        assert_eq!(provider.protocol, Protocol::OpenAiChat);
        assert_eq!(provider.default_model, "auto");
        assert!(provider.models.iter().any(|model| model.id == "auto"));
        assert_eq!(config.default_provider_id, Some(1));
    }

    #[test]
    fn explicit_providers_and_active_selection_survive_round_trip() {
        let mut config = Config {
            providers: vec![
                ProviderProfile::new(
                    3,
                    "Alpha",
                    Protocol::AnthropicMessages,
                    "https://api.anthropic.com/v1",
                    AuthMode::XApiKey,
                    "claude-sonnet",
                ),
                ProviderProfile::new(
                    7,
                    "Beta",
                    Protocol::OpenAiChat,
                    "https://beta.example/v1",
                    AuthMode::Bearer,
                    "beta",
                ),
            ],
            default_provider_id: Some(7),
            ..Config::default()
        };
        config.normalize();
        let path = std::env::temp_dir().join(format!(
            "nexus-config-{}-providers/config.toml",
            std::process::id()
        ));

        config.save(&path).unwrap();
        let loaded = Config::load_or_default(&path).unwrap();

        assert_eq!(loaded.providers, config.providers);
        assert_eq!(loaded.default_provider_id, Some(7));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn invalid_active_provider_falls_back_to_the_first_entry() {
        let mut config = Config {
            providers: vec![ProviderProfile::new(
                9,
                "Only",
                Protocol::OllamaChat,
                "http://127.0.0.1:11434",
                AuthMode::None,
                "llama3.2",
            )],
            default_provider_id: Some(77),
            ..Config::default()
        };
        config.normalize();

        assert_eq!(config.default_provider_id, Some(9));
    }

    #[test]
    fn removing_the_default_provider_selects_another_and_keeps_one() {
        let mut config = Config {
            providers: vec![
                ProviderProfile::new(
                    1,
                    "One",
                    Protocol::OpenAiChat,
                    "https://one.example/v1",
                    AuthMode::Bearer,
                    "one",
                ),
                ProviderProfile::new(
                    2,
                    "Two",
                    Protocol::OpenAiChat,
                    "https://two.example/v1",
                    AuthMode::Bearer,
                    "two",
                ),
            ],
            default_provider_id: Some(1),
            ..Config::default()
        };
        config.normalize();

        assert!(config.remove_provider(1));
        assert_eq!(config.default_provider_id, Some(2));
        assert!(!config.remove_provider(2));
    }

    #[test]
    fn removed_provider_ids_are_not_reused() {
        let mut config = Config::default();
        let second = config.allocate_provider_id();
        assert_eq!(second, 2);
        config.providers.push(ProviderProfile::new(
            second,
            "Second",
            Protocol::OpenAiChat,
            "https://second.example/v1",
            AuthMode::Bearer,
            "second",
        ));
        assert!(config.remove_provider(second));
        assert_eq!(config.allocate_provider_id(), 3);
    }

    #[test]
    fn duplicate_ids_are_repaired_without_losing_entries() {
        let mut config = Config {
            providers: vec![
                ProviderProfile::new(
                    1,
                    "One",
                    Protocol::OpenAiChat,
                    "https://one.example/v1",
                    AuthMode::Bearer,
                    "one",
                ),
                ProviderProfile::new(
                    1,
                    "Two",
                    Protocol::OpenAiChat,
                    "https://two.example/v1",
                    AuthMode::Bearer,
                    "two",
                ),
            ],
            default_provider_id: Some(1),
            ..Config::default()
        };
        config.normalize();

        assert_eq!(config.providers.len(), 2);
        assert_eq!(config.providers[0].id, 1);
        assert_eq!(config.providers[1].id, 2);
    }

    #[test]
    fn invalid_auth_is_replaced_with_the_protocol_default() {
        let mut config = Config {
            providers: vec![ProviderProfile {
                id: 1,
                name: "Anthropic".into(),
                protocol: Protocol::AnthropicMessages,
                base_url: "https://api.anthropic.com/v1".into(),
                auth: AuthMode::None,
                default_model: "claude".into(),
                models: vec![ModelConfig::new("claude")],
            }],
            ..Config::default()
        };
        config.normalize();
        assert_eq!(config.providers[0].auth, AuthMode::XApiKey);
    }

    #[test]
    fn theme_and_history_flags_survive_a_round_trip_without_losing_providers() {
        let mut config = Config {
            theme: Theme::Dark,
            history_enabled: false,
            ..Config::default()
        };
        config.providers[0].name = "Kept provider".to_owned();
        let path = std::env::temp_dir().join(format!(
            "nexus-config-{}-theme/config.toml",
            std::process::id()
        ));
        config.save(&path).unwrap();
        let loaded = Config::load_or_default(&path).unwrap();
        assert_eq!(loaded.theme, Theme::Dark);
        assert!(!loaded.history_enabled);
        assert_eq!(loaded.providers[0].name, "Kept provider");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn mcp_servers_round_trip_through_the_config_file() {
        let mut config = Config::default();
        config.providers.clear();
        config.providers.push(ProviderProfile::new(
            1,
            "Local gateway",
            Protocol::OpenAiChat,
            "http://127.0.0.1:3001/v1",
            AuthMode::None,
            "auto",
        ));
        config.mcp_servers.push(McpServerConfig {
            id: 4,
            name: "Files".to_owned(),
            transport: McpTransport::Stdio,
            command: "npx".to_owned(),
            args: vec!["-y".to_owned(), "server-files".to_owned()],
            cwd: None,
            env_allowlist: vec!["HOME".to_owned()],
            url: String::new(),
            bearer_auth: false,
            enabled_tools: Vec::new(),
            enabled: true,
        });
        let path = std::env::temp_dir().join(format!(
            "nexus-config-{}-mcp/config.toml",
            std::process::id()
        ));
        config.save(&path).unwrap();
        let loaded = Config::load_or_default(&path).unwrap();
        assert_eq!(loaded.mcp_servers.len(), 1);
        assert_eq!(loaded.mcp_servers[0].transport, McpTransport::Stdio);
        assert_eq!(loaded.mcp_servers[0].command, "npx");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
