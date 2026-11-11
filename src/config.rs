use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub welcome_message: String,
    pub provider_base_url: String,
    pub provider_model: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            welcome_message: "Your local agent workspace is ready to configure.".to_owned(),
            provider_base_url: "http://127.0.0.1:3001/v1".to_owned(),
            provider_model: "auto".to_owned(),
        }
    }
}

impl Config {
    pub fn load_or_default(path: &Path) -> Result<Self, AppError> {
        match fs::read_to_string(path) {
            Ok(contents) => toml::from_str(&contents).map_err(|source| AppError::ConfigParse {
                path: path.to_owned(),
                source,
            }),
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
        fs::write(path, contents).map_err(|source| AppError::ConfigWrite {
            path: path.to_owned(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn defaults_provide_a_welcome_message() {
        assert!(!Config::default().welcome_message.is_empty());
    }

    #[test]
    fn parses_a_toml_override_and_defaults_missing_fields() {
        let config: Config = toml::from_str("welcome_message = 'Hello, NEXUS'").unwrap();

        assert_eq!(config.welcome_message, "Hello, NEXUS");
        assert_eq!(config.provider_model, "auto");
    }

    #[test]
    fn missing_config_file_uses_defaults() {
        let path =
            std::env::temp_dir().join(format!("nexus-config-{}-missing.toml", std::process::id()));

        let config = Config::load_or_default(&path).unwrap();

        assert_eq!(config, Config::default());
    }

    #[test]
    fn saves_and_loads_a_config_file() {
        let path = std::env::temp_dir().join(format!(
            "nexus-config-{}-roundtrip/config.toml",
            std::process::id()
        ));
        let config = Config {
            welcome_message: "Hello from a saved config".to_owned(),
            ..Config::default()
        };

        config.save(&path).unwrap();
        let loaded = Config::load_or_default(&path).unwrap();

        assert_eq!(loaded, config);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
