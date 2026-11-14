mod chat;
mod config;
mod credentials;
mod error;
pub mod mcp;
pub mod mission;
mod provider;
pub mod storage;
#[cfg(feature = "telemetry")]
mod telemetry;
mod ui;

use std::path::PathBuf;

use error::AppError;
use tracing_subscriber::EnvFilter;

fn main() -> Result<(), AppError> {
    init_logging();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        pid = std::process::id(),
        "Starting NEXUS"
    );

    let config_path = app_support_path("config.toml")?;
    let database_path = app_support_path("nexus.db")?;
    ui::run(config_path, database_path)?;
    Ok(())
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

fn app_support_path(file_name: &str) -> Result<PathBuf, AppError> {
    let home = std::env::var_os("HOME").ok_or(AppError::MissingHomeDirectory)?;
    #[cfg(target_os = "macos")]
    let root = PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join("NEXUS");
    #[cfg(not(target_os = "macos"))]
    let root = PathBuf::from(home).join(".config").join("nexus");
    Ok(root.join(file_name))
}
