mod config;
mod error;
#[cfg(feature = "telemetry")]
mod telemetry;
mod ui;

use std::path::PathBuf;

use config::Config;
use error::AppError;
use tracing_subscriber::EnvFilter;

fn main() -> Result<(), AppError> {
    init_logging();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        pid = std::process::id(),
        "Starting NEXUS"
    );

    let config_path = config_path()?;
    let config = Config::load_or_default(&config_path)?;
    ui::run(config, config_path)?;
    Ok(())
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

fn config_path() -> Result<PathBuf, AppError> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").ok_or(AppError::MissingHomeDirectory)?;
        Ok(PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("NEXUS")
            .join("config.toml"))
    }

    #[cfg(not(target_os = "macos"))]
    {
        let home = std::env::var_os("HOME").ok_or(AppError::MissingHomeDirectory)?;
        Ok(PathBuf::from(home)
            .join(".config")
            .join("nexus")
            .join("config.toml"))
    }
}
