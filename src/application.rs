//! Exact, nonrecursive checks for supported macOS agent application bundles.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationId {
    DeepAiCode,
    OpenCodeDesktop,
}

impl ApplicationId {
    pub const ALL: [Self; 2] = [Self::DeepAiCode, Self::OpenCodeDesktop];

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::DeepAiCode => "DeepAI Code",
            Self::OpenCodeDesktop => "OpenCode Desktop",
        }
    }

    pub const fn bundle_name(self) -> &'static str {
        match self {
            Self::DeepAiCode => "DeepAI Code.app",
            Self::OpenCodeDesktop => "OpenCode.app",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationStatus {
    Found,
    NotFoundInCheckedApplications,
    Inaccessible,
    SkippedSymlink,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApplicationDetection {
    pub id: ApplicationId,
    pub display_name: String,
    pub bundle_name: String,
    pub status: ApplicationStatus,
    pub path: Option<PathBuf>,
    pub checked_roots: Vec<PathBuf>,
    pub last_discovered_at_unix_secs: u64,
}

/// Checks only the exact candidate bundle paths directly inside these roots.
/// It never enumerates applications or reads bundle contents.
pub fn detect_applications(roots: &[PathBuf], now: u64) -> Vec<ApplicationDetection> {
    ApplicationId::ALL
        .into_iter()
        .map(|id| {
            let mut found = None;
            let mut inaccessible = false;
            let mut skipped_symlink = false;
            for root in roots {
                match fs::symlink_metadata(root) {
                    Ok(root_metadata) if root_metadata.file_type().is_symlink() => {
                        skipped_symlink = true;
                        continue;
                    }
                    Ok(root_metadata) if root_metadata.is_dir() => {}
                    Ok(_) => continue,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(_) => {
                        inaccessible = true;
                        continue;
                    }
                }
                let path = root.join(id.bundle_name());
                match fs::symlink_metadata(&path) {
                    Ok(metadata) if metadata.file_type().is_symlink() => skipped_symlink = true,
                    Ok(metadata) if metadata.is_dir() => {
                        found = Some(path);
                        break;
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => inaccessible = true,
                }
            }
            let status = if found.is_some() {
                ApplicationStatus::Found
            } else if inaccessible {
                ApplicationStatus::Inaccessible
            } else if skipped_symlink {
                ApplicationStatus::SkippedSymlink
            } else {
                ApplicationStatus::NotFoundInCheckedApplications
            };
            ApplicationDetection {
                id,
                display_name: id.display_name().to_owned(),
                bundle_name: id.bundle_name().to_owned(),
                status,
                path: found,
                checked_roots: roots.to_vec(),
                last_discovered_at_unix_secs: now,
            }
        })
        .collect()
}

pub fn macos_application_roots(home: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("/Applications")];
    if let Some(home) = home {
        roots.push(home.join("Applications"));
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_exact_application_bundle_name_without_scanning_bundle_contents() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp = std::env::temp_dir().join(format!(
            "nexus-app-detection-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(temp.join("DeepAI Code.app")).unwrap();
        let found = detect_applications(std::slice::from_ref(&temp), 42);
        let deepai = found
            .iter()
            .find(|item| item.id == ApplicationId::DeepAiCode)
            .unwrap();
        assert_eq!(deepai.status, ApplicationStatus::Found);
        assert_eq!(
            deepai.path.as_deref(),
            Some(temp.join("DeepAI Code.app").as_path())
        );
        assert_eq!(
            found
                .iter()
                .find(|item| item.id == ApplicationId::OpenCodeDesktop)
                .unwrap()
                .status,
            ApplicationStatus::NotFoundInCheckedApplications
        );
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn application_roots_are_limited_to_macos_applications_folders() {
        assert_eq!(
            macos_application_roots(Some(Path::new("/Users/example"))),
            vec![
                PathBuf::from("/Applications"),
                PathBuf::from("/Users/example/Applications")
            ]
        );
    }
}
