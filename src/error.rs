use std::io;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("could not read configuration at {path}")]
    ConfigRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not parse configuration at {path}")]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("could not serialize configuration")]
    ConfigSerialize(#[from] toml::ser::Error),
    #[error("could not write configuration at {path}")]
    ConfigWrite {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not locate the user's home directory")]
    MissingHomeDirectory,
    #[error(transparent)]
    Storage(#[from] crate::storage::StorageError),
    #[error("could not start the native application window")]
    Window(#[from] eframe::Error),
}
