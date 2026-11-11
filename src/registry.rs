use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::agent::{AgentId, AgentRecord, Knowledge};
use crate::application::ApplicationDetection;
use crate::discovery::{DiscoveryScope, LocationResult, LocationStatus, ScanReport};

const REGISTRY_FILE_NAME: &str = "registry.toml";
const REGISTRY_SCHEMA_VERSION: u32 = 2;
const MAX_REGISTRY_BYTES: u64 = 64 * 1024;
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("registry path is not a safe absolute file path or contains a symlink")]
    UnsafePath,
    #[error("registry file exceeds the {MAX_REGISTRY_BYTES} byte limit")]
    TooLarge,
    #[error("registry schema version {0} is not supported")]
    UnsupportedSchema(u32),
    #[error("could not read or write the registry at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not parse the NEXUS registry")]
    Parse(#[from] toml::de::Error),
    #[error("could not serialize the NEXUS registry")]
    Serialize(#[from] toml::ser::Error),
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AgentRegistry {
    schema_version: u32,
    pub records: Vec<AgentRecord>,
    #[serde(default)]
    pub applications: Vec<ApplicationDetection>,
    #[serde(default)]
    pub last_scan_unix_secs: Option<u64>,
    #[serde(default)]
    pub last_search_roots: Vec<PathBuf>,
    #[serde(default)]
    pub last_skipped_roots: Vec<PathBuf>,
    #[serde(default)]
    pub last_location_results: Vec<LocationResult>,
    #[serde(default)]
    pub custom_search_locations: Vec<PathBuf>,
    #[serde(default)]
    pub discovery_scope: DiscoveryScope,
    #[serde(default)]
    pub expanded_scope_approval_version: Option<u32>,
}

impl AgentRegistry {
    pub fn empty() -> Self {
        Self {
            schema_version: REGISTRY_SCHEMA_VERSION,
            ..Self::default()
        }
    }

    pub fn load_or_default(path: &Path) -> Result<Self, RegistryError> {
        validate_destination(path, false)?;
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::empty()),
            Err(source) => {
                return Err(RegistryError::Io {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(RegistryError::UnsafePath);
        }
        if metadata.len() > MAX_REGISTRY_BYTES {
            return Err(RegistryError::TooLarge);
        }
        let contents = fs::read_to_string(path).map_err(|source| RegistryError::Io {
            path: path.to_owned(),
            source,
        })?;
        let registry: Self = toml::from_str(&contents)?;
        if !matches!(registry.schema_version, 1 | REGISTRY_SCHEMA_VERSION) {
            return Err(RegistryError::UnsupportedSchema(registry.schema_version));
        }
        let mut registry = registry;
        registry.schema_version = REGISTRY_SCHEMA_VERSION;
        Ok(registry)
    }

    pub fn merge_scan(&mut self, report: ScanReport) {
        let previous = std::mem::take(&mut self.records);
        self.records = report
            .records
            .into_iter()
            .map(|mut record| {
                if let Some(existing) = previous.iter().find(|item| item.id == record.id) {
                    record.ignored = existing.ignored;
                    if !matches!(existing.installed_version, Knowledge::Unknown) {
                        record.installed_version = existing.installed_version.clone();
                    }
                }
                record
            })
            .collect();
        self.schema_version = REGISTRY_SCHEMA_VERSION;
        self.applications = report.applications;
        self.last_scan_unix_secs = Some(report.completed_at_unix_secs);
        self.last_search_roots = report
            .locations
            .iter()
            .filter(|item| item.status == LocationStatus::Checked)
            .map(|item| item.path.clone())
            .collect();
        self.last_skipped_roots = report
            .locations
            .iter()
            .filter(|item| {
                matches!(
                    item.status,
                    LocationStatus::Skipped
                        | LocationStatus::Inaccessible
                        | LocationStatus::Excluded
                )
            })
            .map(|item| item.path.clone())
            .collect();
        self.last_location_results = report.locations;
        self.discovery_scope = report.scope;
    }

    pub fn add_custom_location(&mut self, path: PathBuf) -> bool {
        if self.custom_search_locations.len() >= 16 || self.custom_search_locations.contains(&path)
        {
            return false;
        }
        self.custom_search_locations.push(path);
        true
    }

    pub fn remove_custom_location(&mut self, path: &Path) {
        self.custom_search_locations
            .retain(|location| location != path);
    }

    pub fn set_ignored(&mut self, id: AgentId, ignored: bool) {
        if let Some(record) = self.records.iter_mut().find(|record| record.id == id) {
            record.ignored = ignored;
        }
    }

    pub fn record_user_reported_version(&mut self, id: AgentId, value: String, recorded_at: u64) {
        if let Some(record) = self.records.iter_mut().find(|record| record.id == id) {
            record.installed_version = Knowledge::UserReported {
                value,
                evidence: crate::agent::Evidence {
                    description: "Version entered by the user after separate verification"
                        .to_owned(),
                    source: None,
                    observed_at_unix_secs: Some(recorded_at),
                },
            };
        }
    }

    pub fn save_atomically(&self, path: &Path) -> Result<(), RegistryError> {
        validate_destination(path, true)?;
        let contents = toml::to_string_pretty(self)?;
        if contents.len() as u64 > MAX_REGISTRY_BYTES {
            return Err(RegistryError::TooLarge);
        }
        let parent = path.parent().ok_or(RegistryError::UnsafePath)?;
        let file_name = path.file_name().ok_or(RegistryError::UnsafePath)?;
        let mut temp_path = None;
        let mut temp_file = None;

        for _ in 0..8 {
            let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let candidate = parent.join(format!(
                ".{REGISTRY_FILE_NAME}.tmp-{}-{sequence}",
                std::process::id()
            ));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            match options.open(&candidate) {
                Ok(file) => {
                    temp_path = Some(candidate);
                    temp_file = Some(file);
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(RegistryError::Io {
                        path: candidate,
                        source,
                    });
                }
            }
        }

        let temp_path = temp_path.ok_or(RegistryError::UnsafePath)?;
        let mut temp_file = temp_file.ok_or(RegistryError::UnsafePath)?;
        let write_result = temp_file
            .write_all(contents.as_bytes())
            .and_then(|()| temp_file.sync_all());
        drop(temp_file);
        if let Err(source) = write_result {
            let _ = fs::remove_file(&temp_path);
            return Err(RegistryError::Io {
                path: temp_path,
                source,
            });
        }

        if let Err(error) = validate_destination(path, false) {
            let _ = fs::remove_file(&temp_path);
            return Err(error);
        }
        let target_meta = fs::symlink_metadata(path);
        match target_meta {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                let _ = fs::remove_file(&temp_path);
                return Err(RegistryError::UnsafePath);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                let _ = fs::remove_file(&temp_path);
                return Err(RegistryError::Io {
                    path: path.to_owned(),
                    source,
                });
            }
        }

        if let Err(source) = fs::rename(&temp_path, parent.join(file_name)) {
            let _ = fs::remove_file(&temp_path);
            return Err(RegistryError::Io {
                path: path.to_owned(),
                source,
            });
        }
        let _ = File::open(parent).and_then(|directory| directory.sync_all());
        Ok(())
    }
}

fn validate_destination(path: &Path, create_parent: bool) -> Result<(), RegistryError> {
    if !path.is_absolute()
        || path.file_name().and_then(|name| name.to_str()) != Some(REGISTRY_FILE_NAME)
    {
        return Err(RegistryError::UnsafePath);
    }
    let parent = path.parent().ok_or(RegistryError::UnsafePath)?;
    if create_parent {
        ensure_directories(parent)?;
    }
    match directory_components_are_safe(parent)? {
        DirectoryStatus::Safe => {}
        DirectoryStatus::Missing if !create_parent => {}
        DirectoryStatus::Missing | DirectoryStatus::Unsafe => {
            return Err(RegistryError::UnsafePath);
        }
    }
    Ok(())
}

fn ensure_directories(path: &Path) -> Result<(), RegistryError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => current.push("/"),
            Component::Normal(part) => {
                current.push(part);
                match fs::symlink_metadata(&current) {
                    Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                    Ok(_) => return Err(RegistryError::UnsafePath),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        fs::create_dir(&current).map_err(|source| RegistryError::Io {
                            path: current.clone(),
                            source,
                        })?;
                        #[cfg(unix)]
                        fs::set_permissions(&current, fs::Permissions::from_mode(0o700)).map_err(
                            |source| RegistryError::Io {
                                path: current.clone(),
                                source,
                            },
                        )?;
                    }
                    Err(source) => {
                        return Err(RegistryError::Io {
                            path: current.clone(),
                            source,
                        });
                    }
                }
            }
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) => return Err(RegistryError::UnsafePath),
        }
    }
    Ok(())
}

enum DirectoryStatus {
    Safe,
    Missing,
    Unsafe,
}

fn directory_components_are_safe(path: &Path) -> Result<DirectoryStatus, RegistryError> {
    if !path.is_absolute() {
        return Ok(DirectoryStatus::Unsafe);
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => current.push("/"),
            Component::Normal(part) => {
                current.push(part);
                let metadata = match fs::symlink_metadata(&current) {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        return Ok(DirectoryStatus::Missing);
                    }
                    Err(source) => {
                        return Err(RegistryError::Io {
                            path: current,
                            source,
                        });
                    }
                };
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Ok(DirectoryStatus::Unsafe);
                }
            }
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) => return Ok(DirectoryStatus::Unsafe),
        }
    }
    Ok(DirectoryStatus::Safe)
}

#[cfg(test)]
mod tests {
    use super::{
        AgentRegistry, DiscoveryScope, LocationResult, LocationStatus, REGISTRY_SCHEMA_VERSION,
        RegistryError, ScanReport,
    };
    use crate::agent::{AgentId, DetectionStatus, Knowledge};
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    fn registry_path(label: &str) -> PathBuf {
        fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("nexus-registry-{}-{label}", std::process::id()))
            .join("registry.toml")
    }

    #[test]
    fn registry_round_trips_and_keeps_unknown_values() {
        let path = registry_path("roundtrip");
        let mut registry = AgentRegistry::empty();
        registry
            .records
            .push(crate::agent::AgentRecord::catalog_entry(AgentId::CodexCli));
        registry.records[0].detection_status = DetectionStatus::Found;
        registry.save_atomically(&path).unwrap();

        let loaded = AgentRegistry::load_or_default(&path).unwrap();
        assert_eq!(loaded.records.len(), 1);
        assert!(matches!(
            loaded.records[0].installed_version,
            Knowledge::Unknown
        ));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn missing_registry_directory_loads_an_empty_registry_without_creating_it() {
        let path = registry_path("missing");
        let registry = AgentRegistry::load_or_default(&path).unwrap();

        assert!(registry.records.is_empty());
        assert!(!path.parent().unwrap().exists());
    }

    #[test]
    fn replacing_existing_registry_is_atomic_and_round_trips_new_state() {
        let path = registry_path("replace");
        let initial = AgentRegistry::empty();
        initial.save_atomically(&path).unwrap();
        let mut replacement = AgentRegistry::empty();
        replacement
            .records
            .push(crate::agent::AgentRecord::catalog_entry(AgentId::Pi));
        replacement.save_atomically(&path).unwrap();

        let loaded = AgentRegistry::load_or_default(&path).unwrap();
        assert_eq!(loaded.records.len(), 1);
        assert_eq!(loaded.records[0].id, AgentId::Pi);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn refuses_a_symlink_destination_without_overwriting_its_target() {
        let parent = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("nexus-registry-link-{}", std::process::id()));
        fs::create_dir_all(&parent).unwrap();
        let target = parent.join("actual.toml");
        let destination = parent.join("registry.toml");
        fs::write(&target, "untouched").unwrap();
        symlink(&target, &destination).unwrap();

        assert!(matches!(
            AgentRegistry::empty().save_atomically(&destination),
            Err(RegistryError::UnsafePath)
        ));
        assert_eq!(fs::read_to_string(target).unwrap(), "untouched");
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn refuses_symlinked_parent_components() {
        let base = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("nexus-registry-parent-{}", std::process::id()));
        let real = base.join("real");
        let link = base.join("link");
        fs::create_dir_all(&real).unwrap();
        symlink(&real, &link).unwrap();
        let destination = link.join("registry.toml");

        assert!(matches!(
            AgentRegistry::empty().save_atomically(&destination),
            Err(RegistryError::UnsafePath)
        ));
        assert!(!real.join("registry.toml").exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn merge_preserves_user_version_and_ignore_choice() {
        let mut registry = AgentRegistry::empty();
        registry
            .records
            .push(crate::agent::AgentRecord::catalog_entry(AgentId::CodexCli));
        registry.record_user_reported_version(AgentId::CodexCli, "1.2.3".to_owned(), 9);
        registry.set_ignored(AgentId::CodexCli, true);

        registry.merge_scan(ScanReport {
            records: vec![crate::agent::AgentRecord::catalog_entry(AgentId::CodexCli)],
            applications: vec![],
            locations: vec![LocationResult {
                path: PathBuf::from("/opt/homebrew/bin"),
                status: LocationStatus::Checked,
                reason: "Homebrew root".into(),
            }],
            completed_at_unix_secs: 12,
            scope: DiscoveryScope::Standard,
        });

        let record = &registry.records[0];
        assert!(record.ignored);
        assert!(matches!(
            &record.installed_version,
            Knowledge::UserReported { value, .. } if value == "1.2.3"
        ));
    }

    #[test]
    fn custom_locations_scope_and_location_evidence_persist() {
        let path = registry_path("custom-scope");
        let mut registry = AgentRegistry::empty();
        assert!(registry.add_custom_location(PathBuf::from("/Users/example/bin")));
        registry.discovery_scope = DiscoveryScope::Expanded;
        registry.expanded_scope_approval_version = Some(1);
        registry.last_location_results.push(LocationResult {
            path: PathBuf::from("/Applications"),
            status: LocationStatus::Checked,
            reason: "Expanded root".into(),
        });
        registry.save_atomically(&path).unwrap();
        let loaded = AgentRegistry::load_or_default(&path).unwrap();
        assert_eq!(
            loaded.custom_search_locations,
            registry.custom_search_locations
        );
        assert_eq!(loaded.discovery_scope, DiscoveryScope::Expanded);
        assert_eq!(loaded.expanded_scope_approval_version, Some(1));
        assert_eq!(loaded.last_location_results, registry.last_location_results);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn version_one_registry_migrates_with_standard_scope_and_no_extra_paths() {
        let path = registry_path("v1-migration");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "schema_version = 1\nrecords = []\n").unwrap();
        let loaded = AgentRegistry::load_or_default(&path).unwrap();
        assert_eq!(loaded.discovery_scope, DiscoveryScope::Standard);
        assert!(loaded.custom_search_locations.is_empty());
        assert_eq!(loaded.schema_version, REGISTRY_SCHEMA_VERSION);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn removing_custom_location_changes_registry_state() {
        let mut registry = AgentRegistry::empty();
        let path = PathBuf::from("/Users/example/tool-bin");
        assert!(registry.add_custom_location(path.clone()));
        registry.remove_custom_location(&path);
        assert!(registry.custom_search_locations.is_empty());
    }

    #[test]
    fn corrupted_registry_is_reported_instead_of_replaced_silently() {
        let path = registry_path("corrupt");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "this is not valid TOML = =").unwrap();
        assert!(matches!(
            AgentRegistry::load_or_default(&path),
            Err(RegistryError::Parse(_))
        ));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
