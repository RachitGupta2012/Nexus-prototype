use std::collections::{HashSet, VecDeque};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::agent::{AgentId, AgentRecord, DetectionStatus, ExecutableLocation};
use crate::application::{ApplicationDetection, detect_applications, macos_application_roots};

const MAX_PATH_ENTRIES: usize = 64;
const MAX_PATH_BYTES: usize = 16 * 1024;
const MAX_SYMLINK_DEPTH: usize = 8;
const MAX_CUSTOM_LOCATIONS: usize = 16;
const MAX_EXPANDED_DEPTH: usize = 4;
const MAX_EXPANDED_DIRECTORIES: usize = 128;
const MAX_EXPANDED_ENTRIES: usize = 3_000;
const MAX_EXPANDED_DURATION: Duration = Duration::from_secs(10);
const MAX_DIAGNOSTICS: usize = 256;
pub const EXPANDED_SCOPE_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryScope {
    #[default]
    Standard,
    Expanded,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchRoot {
    pub directory: PathBuf,
    pub containment_root: PathBuf,
    pub label: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocationStatus {
    Checked,
    Missing,
    Skipped,
    Inaccessible,
    Excluded,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LocationResult {
    pub path: PathBuf,
    pub status: LocationStatus,
    pub reason: String,
}

#[derive(Clone, Debug, Default)]
pub struct SearchPlan {
    pub roots: Vec<SearchRoot>,
    pub initial_results: Vec<LocationResult>,
}

#[derive(Clone, Debug)]
pub struct ScanProgress {
    pub scope: DiscoveryScope,
    pub current_location: Option<PathBuf>,
    pub locations_checked: usize,
    pub entries_examined: usize,
}

#[derive(Clone, Debug)]
pub struct ScanReport {
    pub records: Vec<AgentRecord>,
    pub applications: Vec<ApplicationDetection>,
    pub locations: Vec<LocationResult>,
    pub completed_at_unix_secs: u64,
    pub scope: DiscoveryScope,
}

#[derive(Clone, Debug)]
pub enum ScanOutcome {
    Complete(ScanReport),
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScanState {
    Idle,
    Scanning,
    Cancelling,
    Failed(String),
}

struct ScanWorker {
    cancelled: Arc<AtomicBool>,
    receiver: Receiver<ScanOutcome>,
    worker: JoinHandle<()>,
}

pub struct DiscoveryController {
    worker: Option<ScanWorker>,
    state: ScanState,
    progress: Arc<Mutex<ScanProgress>>,
}

impl Default for DiscoveryController {
    fn default() -> Self {
        Self {
            worker: None,
            state: ScanState::Idle,
            progress: Arc::new(Mutex::new(ScanProgress {
                scope: DiscoveryScope::Standard,
                current_location: None,
                locations_checked: 0,
                entries_examined: 0,
            })),
        }
    }
}

impl DiscoveryController {
    pub fn state(&self) -> &ScanState {
        &self.state
    }

    pub fn progress(&self) -> ScanProgress {
        self.progress.lock().map_or_else(
            |_| ScanProgress {
                scope: DiscoveryScope::Standard,
                current_location: None,
                locations_checked: 0,
                entries_examined: 0,
            },
            |progress| progress.clone(),
        )
    }

    pub fn start(&mut self, plan: SearchPlan, scope: DiscoveryScope) {
        if self.worker.is_some() {
            return;
        }
        if let Ok(mut progress) = self.progress.lock() {
            *progress = ScanProgress {
                scope,
                current_location: None,
                locations_checked: 0,
                entries_examined: 0,
            };
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let worker_progress = Arc::clone(&self.progress);
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("nexus-agent-discovery".to_owned())
            .spawn(move || {
                let outcome = scan(&plan, scope, &worker_cancelled, &worker_progress);
                let _ = sender.send(outcome);
            });

        match worker {
            Ok(worker) => {
                self.worker = Some(ScanWorker {
                    cancelled,
                    receiver,
                    worker,
                });
                self.state = ScanState::Scanning;
            }
            Err(error) => self.state = ScanState::Failed(format!("Could not start scan: {error}")),
        }
    }

    pub fn cancel(&mut self) {
        if let Some(worker) = &self.worker {
            worker.cancelled.store(true, Ordering::Relaxed);
            self.state = ScanState::Cancelling;
        }
    }

    pub fn poll(&mut self) -> Option<ScanOutcome> {
        let worker = self.worker.as_ref()?;
        match worker.receiver.try_recv() {
            Ok(outcome) => {
                if let Some(worker) = self.worker.take() {
                    let _ = worker.worker.join();
                }
                self.state = ScanState::Idle;
                Some(outcome)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                if let Some(worker) = self.worker.take() {
                    let _ = worker.worker.join();
                }
                self.state = ScanState::Failed("Scan stopped without a result".to_owned());
                None
            }
        }
    }
}

impl Drop for DiscoveryController {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.cancelled.store(true, Ordering::Relaxed);
            let _ = worker.worker.join();
        }
    }
}

pub fn standard_search_plan(
    home: Option<&Path>,
    path: Option<&std::ffi::OsStr>,
    custom_locations: &[PathBuf],
) -> SearchPlan {
    let mut plan = SearchPlan::default();
    let mut seen = HashSet::new();

    if let Some(path) = path {
        if path.as_encoded_bytes().len() > MAX_PATH_BYTES {
            plan.initial_results.push(LocationResult {
                path: PathBuf::from("PATH"),
                status: LocationStatus::Skipped,
                reason: format!("PATH exceeded the {MAX_PATH_BYTES}-byte scan limit"),
            });
        } else {
            let mut entries = std::env::split_paths(path);
            for directory in entries.by_ref().take(MAX_PATH_ENTRIES) {
                match containment_root(&directory, home) {
                    Some(root) => {
                        add_root(&mut plan, &mut seen, directory, root, "eligible PATH entry")
                    }
                    None => plan.initial_results.push(LocationResult {
                        path: directory,
                        status: LocationStatus::Skipped,
                        reason: "PATH entry is relative or outside documented local roots".into(),
                    }),
                }
            }
            let skipped_count = entries.count();
            if skipped_count > 0 {
                plan.initial_results.push(LocationResult {
                    path: PathBuf::from("PATH entries beyond the scan limit"),
                    status: LocationStatus::Skipped,
                    reason: format!(
                        "{skipped_count} entries beyond the first {MAX_PATH_ENTRIES} were not checked"
                    ),
                });
            }
        }
    }

    for (directory, root, label) in conventional_roots(home) {
        add_root(&mut plan, &mut seen, directory, root, label);
    }

    for directory in custom_locations.iter().take(MAX_CUSTOM_LOCATIONS) {
        match validate_custom_location(directory) {
            Ok(canonical) => add_root(
                &mut plan,
                &mut seen,
                canonical.clone(),
                canonical,
                "user-authorized custom location",
            ),
            Err(reason) => plan.initial_results.push(LocationResult {
                path: directory.clone(),
                status: LocationStatus::Skipped,
                reason,
            }),
        }
    }
    for directory in custom_locations.iter().skip(MAX_CUSTOM_LOCATIONS) {
        plan.initial_results.push(LocationResult {
            path: directory.clone(),
            status: LocationStatus::Skipped,
            reason: format!("only {MAX_CUSTOM_LOCATIONS} custom locations are supported"),
        });
    }
    plan
}

pub fn expanded_search_roots(home: Option<&Path>) -> Vec<SearchRoot> {
    let mut roots = vec![
        SearchRoot {
            directory: PathBuf::from("/Applications"),
            containment_root: PathBuf::from("/Applications"),
            label: "Expanded: system Applications directory".into(),
        },
        SearchRoot {
            directory: PathBuf::from("/opt"),
            containment_root: PathBuf::from("/opt"),
            label: "Expanded: optional software installation directory".into(),
        },
    ];
    if let Some(home) = home {
        roots.push(SearchRoot {
            directory: home.join("Applications"),
            containment_root: home.to_path_buf(),
            label: "Expanded: user's Applications directory".into(),
        });
    }
    roots
}

fn conventional_roots(home: Option<&Path>) -> Vec<(PathBuf, PathBuf, &'static str)> {
    let mut roots = vec![
        (
            "/opt/homebrew/bin",
            "/opt/homebrew",
            "Homebrew on Apple Silicon",
        ),
        (
            "/usr/local/bin",
            "/usr/local",
            "Homebrew and local CLI binaries",
        ),
        ("/usr/bin", "/usr", "macOS system PATH binaries"),
    ]
    .into_iter()
    .map(|(directory, root, label)| (directory.into(), root.into(), label))
    .collect::<Vec<_>>();

    if let Some(home) = home {
        for (relative, label) in [
            ("bin", "Common user CLI directory"),
            (".local/bin", "XDG user CLI directory"),
            (".cargo/bin", "Cargo-installed binaries"),
            (".npm/bin", "npm user-global binary directory"),
            (".npm-global/bin", "npm custom-prefix binary directory"),
            (".bun/bin", "Bun-installed binaries"),
            (".volta/bin", "Volta shim binaries"),
            (".local/share/pnpm", "pnpm user-global binary directory"),
            ("Library/pnpm", "pnpm macOS user-global binary directory"),
            (".linuxbrew/bin", "Home-directory Homebrew binaries"),
            (".asdf/shims", "asdf-managed command shims"),
            (".local/share/mise/shims", "mise-managed command shims"),
            (".yarn/bin", "Yarn user-level binaries"),
        ] {
            let directory = home.join(relative);
            let root = directory
                .parent()
                .filter(|_| relative.contains('/'))
                .unwrap_or(home)
                .to_path_buf();
            roots.push((directory, root, label));
        }
    }
    roots
}

fn containment_root(path: &Path, home: Option<&Path>) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    for root in ["/opt/homebrew", "/usr/local", "/usr"] {
        let root_path = Path::new(root);
        if path.starts_with(root_path) {
            return Some(root_path.to_path_buf());
        }
    }
    home.and_then(|home| {
        if !path.starts_with(home) {
            return None;
        }
        let relative = path.strip_prefix(home).ok()?;
        let eligible = [
            "bin",
            ".local",
            ".cargo",
            ".npm",
            ".npm-global",
            ".bun",
            ".volta",
            ".linuxbrew",
            ".nvm",
            ".fnm",
            ".asdf",
            ".yarn",
            "Library/pnpm",
        ];
        eligible
            .iter()
            .any(|root| relative.starts_with(root))
            .then(|| {
                if relative.starts_with("Library/pnpm") {
                    home.join("Library")
                } else {
                    relative.components().next().map_or_else(
                        || home.to_path_buf(),
                        |component| home.join(component.as_os_str()),
                    )
                }
            })
    })
}

fn add_root(
    plan: &mut SearchPlan,
    seen: &mut HashSet<PathBuf>,
    directory: PathBuf,
    containment_root: PathBuf,
    label: &str,
) {
    if !directory.is_absolute() || !containment_root.is_absolute() {
        plan.initial_results.push(LocationResult {
            path: directory,
            status: LocationStatus::Skipped,
            reason: "search path must be absolute".into(),
        });
        return;
    }
    let Some(directory) = normalize_absolute(&directory) else {
        plan.initial_results.push(LocationResult {
            path: directory,
            status: LocationStatus::Skipped,
            reason: "search path contains an unsafe parent traversal".into(),
        });
        return;
    };
    if seen.insert(directory.clone()) {
        plan.roots.push(SearchRoot {
            directory,
            containment_root,
            label: label.into(),
        });
    } else {
        plan.initial_results.push(LocationResult {
            path: directory,
            status: LocationStatus::Skipped,
            reason: "duplicate location; the same directory is checked once".into(),
        });
    }
}

pub fn validate_custom_location(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("custom location must be an absolute local directory".into());
    }
    let normalized = normalize_absolute(path)
        .ok_or_else(|| "custom location contains an unsafe path component".to_owned())?;
    if is_protected_or_remote_path(&normalized) {
        return Err("system-protected and known remote locations are excluded".into());
    }
    if has_symlink_component(&normalized).map_err(|_| "could not safely inspect path".to_owned())? {
        return Err(
            "custom location contains a symlink; choose its real directory directly".into(),
        );
    }
    let metadata = fs::symlink_metadata(&normalized)
        .map_err(|error| format!("custom location is inaccessible: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("custom location must be an existing directory, not a symlink".into());
    }
    if !filesystem_is_local(&normalized) {
        return Err("custom location is on a remote or unrecognized filesystem".into());
    }
    Ok(normalized)
}

#[cfg(target_os = "macos")]
fn filesystem_is_local(path: &Path) -> bool {
    use nix::sys::statfs::statfs;
    statfs(path).is_ok_and(|info| {
        matches!(
            info.filesystem_type_name().to_ascii_lowercase().as_str(),
            "apfs" | "hfs" | "exfat" | "msdos" | "ufs"
        )
    })
}

#[cfg(not(target_os = "macos"))]
fn filesystem_is_local(_path: &Path) -> bool {
    false
}

fn scan(
    plan: &SearchPlan,
    scope: DiscoveryScope,
    cancelled: &AtomicBool,
    progress: &Mutex<ScanProgress>,
) -> ScanOutcome {
    let mut locations = plan.initial_results.clone();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let application_roots = macos_application_roots(home.as_deref());
    let applications = detect_applications(&application_roots, unix_time_now());
    for root in &application_roots {
        match fs::symlink_metadata(root) {
            Ok(metadata) if metadata.file_type().is_symlink() => locations.push(LocationResult {
                path: root.clone(), status: LocationStatus::Skipped,
                reason: "application search root is a symlink".into(),
            }),
            Ok(metadata) if metadata.is_dir() => locations.push(LocationResult {
                path: root.clone(), status: LocationStatus::Checked,
                reason: "exact supported .app bundle names checked directly; no recursion or bundle contents read".into(),
            }),
            Ok(_) => locations.push(LocationResult {
                path: root.clone(), status: LocationStatus::Skipped,
                reason: "application search root is not a directory".into(),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => locations.push(LocationResult {
                path: root.clone(), status: LocationStatus::Missing,
                reason: "application search root does not exist".into(),
            }),
            Err(_) => locations.push(LocationResult {
                path: root.clone(), status: LocationStatus::Inaccessible,
                reason: "application search root cannot be inspected".into(),
            }),
        }
    }
    let mut roots = plan.roots.clone();
    if scope == DiscoveryScope::Expanded {
        for root in expanded_search_roots(home.as_deref()) {
            if !roots
                .iter()
                .any(|existing| existing.directory == root.directory)
            {
                roots.push(root);
            }
        }
    }
    let mut records: Vec<_> = AgentId::ALL
        .into_iter()
        .map(AgentRecord::catalog_entry)
        .collect();
    let names: Vec<_> = records
        .iter()
        .map(|record| record.executable_name.clone())
        .collect();
    let mut examined_entries = 0;
    let mut expanded_directories = 0;
    let mut locations_checked = 0;
    let expanded_started = Instant::now();

    for root in roots {
        if cancelled.load(Ordering::Relaxed) {
            return ScanOutcome::Cancelled;
        }
        let root_status = validate_search_root(&root);
        match root_status {
            Ok(true) => locations.push(LocationResult {
                path: root.directory.clone(),
                status: LocationStatus::Checked,
                reason: root.label.clone(),
            }),
            Ok(false) => {
                locations.push(LocationResult {
                    path: root.directory,
                    status: LocationStatus::Missing,
                    reason: format!("{} does not exist", root.label),
                });
                continue;
            }
            Err(reason) => {
                let status = if reason.contains("inaccessible") {
                    LocationStatus::Inaccessible
                } else {
                    LocationStatus::Skipped
                };
                locations.push(LocationResult {
                    path: root.directory,
                    status,
                    reason,
                });
                continue;
            }
        }
        locations_checked += 1;
        set_progress(
            progress,
            scope,
            &root.directory,
            locations_checked,
            examined_entries,
        );

        if scope == DiscoveryScope::Standard || !root.label.starts_with("Expanded:") {
            for record in &mut records {
                if cancelled.load(Ordering::Relaxed) {
                    return ScanOutcome::Cancelled;
                }
                let candidate = root.directory.join(record.executable_name.as_str());
                examine_candidate(
                    &candidate,
                    &root.containment_root,
                    cancelled,
                    record,
                    &mut locations,
                );
            }
        } else {
            let mut queue = VecDeque::from([(root.directory.clone(), 0_usize)]);
            while let Some((directory, depth)) = queue.pop_front() {
                if cancelled.load(Ordering::Relaxed) {
                    return ScanOutcome::Cancelled;
                }
                if expanded_directories >= MAX_EXPANDED_DIRECTORIES
                    || examined_entries >= MAX_EXPANDED_ENTRIES
                    || expanded_started.elapsed() >= MAX_EXPANDED_DURATION
                {
                    locations.push(LocationResult {
                        path: directory,
                        status: LocationStatus::Skipped,
                        reason: "Expanded scan reached its depth, entry, directory, or time limit"
                            .into(),
                    });
                    queue.clear();
                    break;
                }
                expanded_directories += 1;
                match validate_search_root(&SearchRoot {
                    directory: directory.clone(),
                    containment_root: root.containment_root.clone(),
                    label: root.label.clone(),
                }) {
                    Ok(true) => {}
                    Ok(false) => {
                        locations.push(LocationResult {
                            path: directory,
                            status: LocationStatus::Missing,
                            reason: "expanded directory disappeared before it was checked".into(),
                        });
                        continue;
                    }
                    Err(reason) => {
                        locations.push(LocationResult {
                            path: directory,
                            status: if reason.contains("inaccessible") {
                                LocationStatus::Inaccessible
                            } else {
                                LocationStatus::Skipped
                            },
                            reason,
                        });
                        continue;
                    }
                }
                if directory != root.directory {
                    locations.push(LocationResult {
                        path: directory.clone(),
                        status: LocationStatus::Checked,
                        reason: format!("Expanded directory enumerated at depth {depth}"),
                    });
                }
                locations_checked += 1;
                set_progress(
                    progress,
                    scope,
                    &directory,
                    locations_checked,
                    examined_entries,
                );
                let entries = match fs::read_dir(&directory) {
                    Ok(entries) => entries,
                    Err(error) => {
                        locations.push(LocationResult {
                            path: directory,
                            status: LocationStatus::Inaccessible,
                            reason: format!("directory could not be listed: {error}"),
                        });
                        continue;
                    }
                };
                for entry in entries {
                    if cancelled.load(Ordering::Relaxed) {
                        return ScanOutcome::Cancelled;
                    }
                    if expanded_started.elapsed() >= MAX_EXPANDED_DURATION
                        || examined_entries >= MAX_EXPANDED_ENTRIES
                    {
                        queue.clear();
                        break;
                    }
                    examined_entries += 1;
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(_) => continue,
                    };
                    let path = entry.path();
                    let name = entry.file_name();
                    if let Some(index) = names
                        .iter()
                        .position(|candidate| name == candidate.as_str())
                    {
                        let record = &mut records[index];
                        examine_candidate(
                            &path,
                            &root.containment_root,
                            cancelled,
                            record,
                            &mut locations,
                        );
                    }
                    let file_type = match entry.file_type() {
                        Ok(file_type) => file_type,
                        Err(_) => continue,
                    };
                    if file_type.is_symlink() {
                        locations.push(LocationResult {
                            path,
                            status: LocationStatus::Skipped,
                            reason: "directory symlink was not traversed".into(),
                        });
                    } else if file_type.is_dir() {
                        if excluded_directory_name(&name) {
                            locations.push(LocationResult {
                                path,
                                status: LocationStatus::Excluded,
                                reason: "personal-data or protected subtree is excluded".into(),
                            });
                        } else if depth >= MAX_EXPANDED_DEPTH {
                            locations.push(LocationResult {
                                path,
                                status: LocationStatus::Skipped,
                                reason: format!(
                                    "maximum traversal depth ({MAX_EXPANDED_DEPTH}) reached"
                                ),
                            });
                        } else {
                            queue.push_back((path, depth + 1));
                        }
                    }
                    set_progress(
                        progress,
                        scope,
                        &directory,
                        locations_checked,
                        examined_entries,
                    );
                }
            }
        }
    }

    if cancelled.load(Ordering::Relaxed) {
        return ScanOutcome::Cancelled;
    }
    for record in &mut records {
        if !record.locations.is_empty() {
            record.detection_status =
                if record.id.basename_is_ambiguous() || record.locations.len() > 1 {
                    DetectionStatus::Ambiguous
                } else {
                    DetectionStatus::Found
                };
        } else {
            let has_checked_scope = locations
                .iter()
                .any(|location| location.status == LocationStatus::Checked);
            let inaccessible_only = locations
                .iter()
                .any(|location| location.status == LocationStatus::Inaccessible);
            record.detection_status = if has_checked_scope {
                DetectionStatus::NotFoundInScope
            } else if inaccessible_only {
                DetectionStatus::Inaccessible
            } else {
                DetectionStatus::Skipped
            };
        }
        record.last_discovered_at_unix_secs = Some(unix_time_now());
    }
    let _ = EXPANDED_SCOPE_VERSION;
    ScanOutcome::Complete(ScanReport {
        records,
        applications,
        locations: bounded_diagnostics(locations),
        completed_at_unix_secs: unix_time_now(),
        scope,
    })
}

fn bounded_diagnostics(mut diagnostics: Vec<LocationResult>) -> Vec<LocationResult> {
    if diagnostics.len() > MAX_DIAGNOSTICS {
        diagnostics.truncate(MAX_DIAGNOSTICS - 1);
        diagnostics.push(LocationResult {
            path: PathBuf::from("discovery report"),
            status: LocationStatus::Skipped,
            reason: format!(
                "additional location details omitted at the {MAX_DIAGNOSTICS}-item report limit"
            ),
        });
    }
    diagnostics
}

fn excluded_directory_name(name: &std::ffi::OsStr) -> bool {
    [
        ".ssh",
        ".gnupg",
        ".aws",
        ".config",
        ".local/share",
        "Documents",
        "Downloads",
        "Desktop",
        "Pictures",
        "Photos",
        "Movies",
        "Music",
        "Library",
        "System",
        "Caches",
        "Application Support",
        "Containers",
        "Mail",
        "Messages",
        "Keychains",
    ]
    .iter()
    .any(|excluded| name == *excluded)
}

fn set_progress(
    progress: &Mutex<ScanProgress>,
    scope: DiscoveryScope,
    path: &Path,
    locations_checked: usize,
    entries_examined: usize,
) {
    if let Ok(mut current) = progress.lock() {
        current.scope = scope;
        current.current_location = Some(path.to_path_buf());
        current.locations_checked = locations_checked;
        current.entries_examined = entries_examined;
    }
}

fn validate_search_root(root: &SearchRoot) -> Result<bool, String> {
    if is_protected_or_remote_path(&root.directory)
        || is_protected_or_remote_path(&root.containment_root)
    {
        return Err("known protected or remote mount location is excluded".into());
    }
    let directory = normalize_absolute(&root.directory)
        .ok_or_else(|| "invalid absolute search path".to_owned())?;
    let containment_root = normalize_absolute(&root.containment_root)
        .ok_or_else(|| "invalid containment root".to_owned())?;
    if !directory.starts_with(&containment_root)
        || has_symlink_component(&containment_root).unwrap_or(true)
        || has_symlink_component(&directory).unwrap_or(true)
    {
        return Err("search root is unsafe or contains a symlink".into());
    }
    let metadata = match fs::symlink_metadata(&directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err("search root is inaccessible".into());
        }
        Err(error) => return Err(format!("search root metadata unavailable: {error}")),
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        if filesystem_is_local(&directory) {
            Ok(true)
        } else {
            Err("search root is on a remote or unrecognized filesystem".into())
        }
    } else {
        Err("search root is not a real directory".into())
    }
}

fn is_protected_or_remote_path(path: &Path) -> bool {
    ["/Volumes", "/Network", "/net", "/System", "/private/var/db"]
        .iter()
        .any(|prefix| path.starts_with(prefix))
}

fn examine_candidate(
    candidate: &Path,
    root: &Path,
    cancelled: &AtomicBool,
    record: &mut AgentRecord,
    locations: &mut Vec<LocationResult>,
) {
    match resolve_candidate(candidate, root, cancelled) {
        CandidateResult::Found(resolved_path) => record.locations.push(ExecutableLocation {
            discovered_path: candidate.to_path_buf(),
            resolved_path,
            search_root: root.to_path_buf(),
        }),
        CandidateResult::Missing => {}
        CandidateResult::Inaccessible => locations.push(LocationResult {
            path: candidate.to_path_buf(),
            status: LocationStatus::Inaccessible,
            reason: "candidate metadata could not be read or was not executable".into(),
        }),
        CandidateResult::EscapesRoot | CandidateResult::TooManyLinks => {
            locations.push(LocationResult {
                path: candidate.to_path_buf(),
                status: LocationStatus::Skipped,
                reason: "candidate symlink escapes its authorized root or exceeds link limit"
                    .into(),
            })
        }
        CandidateResult::Cancelled => {}
    }
}

fn has_symlink_component(path: &Path) -> Result<bool, ()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => current.push("/"),
            Component::Normal(part) => {
                current.push(part);
                let metadata = fs::symlink_metadata(&current).map_err(|_| ())?;
                if metadata.file_type().is_symlink() {
                    return Ok(true);
                }
            }
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) => return Err(()),
        }
    }
    Ok(false)
}

enum CandidateResult {
    Found(PathBuf),
    Missing,
    Inaccessible,
    EscapesRoot,
    TooManyLinks,
    Cancelled,
}

fn resolve_candidate(candidate: &Path, root: &Path, cancelled: &AtomicBool) -> CandidateResult {
    let Some(mut current) = normalize_absolute(candidate) else {
        return CandidateResult::Inaccessible;
    };
    let Some(root) = normalize_absolute(root) else {
        return CandidateResult::Inaccessible;
    };
    for _ in 0..=MAX_SYMLINK_DEPTH {
        if cancelled.load(Ordering::Relaxed) {
            return CandidateResult::Cancelled;
        }
        let Ok(relative) = current.strip_prefix(&root) else {
            return CandidateResult::EscapesRoot;
        };
        match resolve_components(&root, relative, cancelled) {
            ComponentResolution::Found(path) => return CandidateResult::Found(path),
            ComponentResolution::Missing => return CandidateResult::Missing,
            ComponentResolution::Inaccessible => return CandidateResult::Inaccessible,
            ComponentResolution::EscapesRoot => return CandidateResult::EscapesRoot,
            ComponentResolution::Retry(path) => current = path,
            ComponentResolution::Cancelled => return CandidateResult::Cancelled,
        }
    }
    CandidateResult::TooManyLinks
}

enum ComponentResolution {
    Found(PathBuf),
    Missing,
    Inaccessible,
    EscapesRoot,
    Retry(PathBuf),
    Cancelled,
}

fn resolve_components(root: &Path, relative: &Path, cancelled: &AtomicBool) -> ComponentResolution {
    let mut current = root.to_path_buf();
    let components: Vec<_> = relative.components().collect();
    for (index, component) in components.iter().enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            return ComponentResolution::Cancelled;
        }
        let Component::Normal(part) = component else {
            return ComponentResolution::EscapesRoot;
        };
        current.push(part);
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return ComponentResolution::Missing;
            }
            Err(_) => return ComponentResolution::Inaccessible,
        };
        if metadata.file_type().is_symlink() {
            let target = match fs::read_link(&current) {
                Ok(target) => target,
                Err(_) => return ComponentResolution::Inaccessible,
            };
            let resolved = if target.is_absolute() {
                normalize_absolute(&target)
            } else {
                normalize_absolute(&current.parent().unwrap_or(root).join(target))
            };
            let Some(resolved) = resolved else {
                return ComponentResolution::EscapesRoot;
            };
            if !resolved.starts_with(root) {
                return ComponentResolution::EscapesRoot;
            }
            if index + 1 < components.len() {
                let remainder: PathBuf = components[index + 1..]
                    .iter()
                    .filter_map(|component| match component {
                        Component::Normal(part) => Some(part),
                        _ => None,
                    })
                    .collect();
                return ComponentResolution::Retry(resolved.join(remainder));
            }
            return ComponentResolution::Retry(resolved);
        }
        if index + 1 == components.len() {
            return if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
                ComponentResolution::Found(current)
            } else {
                ComponentResolution::Inaccessible
            };
        }
        if !metadata.is_dir() {
            return ComponentResolution::Inaccessible;
        }
    }
    ComponentResolution::Missing
}

fn normalize_absolute(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => normalized.push("/"),
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() || normalized.as_os_str().is_empty() {
                    return None;
                }
            }
            Component::Prefix(_) => return None,
        }
    }
    Some(normalized)
}

pub fn unix_time_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::{
        DiscoveryScope, LocationStatus, ScanOutcome, SearchRoot, expanded_search_roots, scan,
        standard_search_plan, validate_custom_location,
    };
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;

    fn executable(path: &std::path::Path) {
        fs::write(path, b"fixture, never executed").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn local_temp_root() -> PathBuf {
        fs::canonicalize(std::env::temp_dir()).unwrap()
    }

    #[test]
    fn standard_plan_contains_documented_roots_and_deduplicates_path() {
        let home = PathBuf::from("/Users/example");
        let path = std::ffi::OsStr::new(
            "/opt/homebrew/bin:/opt/homebrew/bin:/Users/example/.bun/bin:/tmp/random",
        );
        let plan = standard_search_plan(Some(&home), Some(path), &[]);
        assert!(
            plan.roots
                .iter()
                .any(|root| root.directory == Path::new("/opt/homebrew/bin"))
        );
        assert!(
            plan.roots
                .iter()
                .any(|root| root.directory == home.join(".bun/bin"))
        );
        assert_eq!(
            plan.roots
                .iter()
                .filter(|root| root.directory == Path::new("/opt/homebrew/bin"))
                .count(),
            1
        );
        assert!(
            plan.initial_results
                .iter()
                .any(|item| item.path == Path::new("/tmp/random")
                    && item.status == LocationStatus::Skipped)
        );
    }

    #[test]
    fn expanded_roots_are_platform_specific_and_bounded() {
        let roots = expanded_search_roots(Some(Path::new("/Users/example")));
        assert!(roots.len() <= 3);
        assert!(roots.iter().all(|root| root.directory.is_absolute()));
        assert!(
            roots
                .iter()
                .any(|root| root.directory == Path::new("/Applications"))
        );
    }

    #[test]
    fn custom_location_rejects_missing_and_symlinked_paths() {
        let base = local_temp_root().join(format!("nexus-custom-{}", std::process::id()));
        let real = base.join("real");
        fs::create_dir_all(&real).unwrap();
        let link = base.join("link");
        symlink(&real, &link).unwrap();
        assert!(validate_custom_location(&link).is_err());
        assert!(validate_custom_location(&base.join("missing")).is_err());
        assert!(validate_custom_location(&real).is_ok());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn scan_finds_exact_names_without_executing_candidates() {
        let root = local_temp_root().join(format!("nexus-discovery-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        executable(&root.join("codex"));
        executable(&root.join("unrelated-agent"));
        let plan = super::SearchPlan {
            roots: vec![SearchRoot {
                directory: root.clone(),
                containment_root: root.clone(),
                label: "fixture".into(),
            }],
            ..Default::default()
        };
        let progress = Mutex::new(super::ScanProgress {
            scope: DiscoveryScope::Standard,
            current_location: None,
            locations_checked: 0,
            entries_examined: 0,
        });
        let ScanOutcome::Complete(report) = scan(
            &plan,
            DiscoveryScope::Standard,
            &AtomicBool::new(false),
            &progress,
        ) else {
            panic!("expected complete scan")
        };
        let codex = report
            .records
            .iter()
            .find(|record| record.id == crate::agent::AgentId::CodexCli)
            .unwrap();
        assert_eq!(codex.detection_status, crate::agent::DetectionStatus::Found);
        assert!(
            report
                .records
                .iter()
                .find(|record| record.id == crate::agent::AgentId::OpenCode)
                .unwrap()
                .locations
                .is_empty()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_symlink_targets_outside_authorized_root() {
        let base = local_temp_root().join(format!("nexus-symlink-{}", std::process::id()));
        let inside = base.join("inside");
        let outside = base.join("outside");
        fs::create_dir_all(&inside).unwrap();
        fs::create_dir_all(&outside).unwrap();
        executable(&outside.join("codex"));
        symlink(outside.join("codex"), inside.join("codex")).unwrap();
        let plan = super::SearchPlan {
            roots: vec![SearchRoot {
                directory: inside.clone(),
                containment_root: inside.clone(),
                label: "fixture".into(),
            }],
            ..Default::default()
        };
        let progress = Mutex::new(super::ScanProgress {
            scope: DiscoveryScope::Standard,
            current_location: None,
            locations_checked: 0,
            entries_examined: 0,
        });
        let ScanOutcome::Complete(report) = scan(
            &plan,
            DiscoveryScope::Standard,
            &AtomicBool::new(false),
            &progress,
        ) else {
            panic!("expected complete scan")
        };
        assert!(
            report
                .locations
                .iter()
                .any(|item| item.status == LocationStatus::Skipped)
        );
        assert!(
            report
                .records
                .iter()
                .find(|record| record.id == crate::agent::AgentId::CodexCli)
                .unwrap()
                .locations
                .is_empty()
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn follows_symlinks_only_inside_authorized_containment_root() {
        let root = local_temp_root().join(format!("nexus-contained-{}", std::process::id()));
        let bin = root.join("bin");
        let cell = root.join("cellar");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&cell).unwrap();
        executable(&cell.join("codex"));
        symlink(PathBuf::from("../cellar/codex"), bin.join("codex")).unwrap();
        let plan = super::SearchPlan {
            roots: vec![SearchRoot {
                directory: bin,
                containment_root: root.clone(),
                label: "fixture".into(),
            }],
            ..Default::default()
        };
        let progress = Mutex::new(super::ScanProgress {
            scope: DiscoveryScope::Standard,
            current_location: None,
            locations_checked: 0,
            entries_examined: 0,
        });
        let ScanOutcome::Complete(report) = scan(
            &plan,
            DiscoveryScope::Standard,
            &AtomicBool::new(false),
            &progress,
        ) else {
            panic!("expected complete scan")
        };
        let codex = report
            .records
            .iter()
            .find(|record| record.id == crate::agent::AgentId::CodexCli)
            .unwrap();
        assert_eq!(codex.detection_status, crate::agent::DetectionStatus::Found);
        assert_eq!(codex.locations[0].resolved_path, cell.join("codex"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn same_executable_from_multiple_roots_is_deduplicated_as_ambiguous() {
        let base = local_temp_root().join(format!("nexus-duplicates-{}", std::process::id()));
        let first = base.join("first");
        let second = base.join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        executable(&first.join("codex"));
        executable(&second.join("codex"));
        let plan = super::SearchPlan {
            roots: [first.clone(), second.clone()]
                .map(|directory| SearchRoot {
                    directory: directory.clone(),
                    containment_root: directory,
                    label: "fixture".into(),
                })
                .into(),
            ..Default::default()
        };
        let progress = Mutex::new(super::ScanProgress {
            scope: DiscoveryScope::Standard,
            current_location: None,
            locations_checked: 0,
            entries_examined: 0,
        });
        let ScanOutcome::Complete(report) = scan(
            &plan,
            DiscoveryScope::Standard,
            &AtomicBool::new(false),
            &progress,
        ) else {
            panic!("expected complete scan")
        };
        let codex = report
            .records
            .iter()
            .find(|record| record.id == crate::agent::AgentId::CodexCli)
            .unwrap();
        assert_eq!(
            codex.detection_status,
            crate::agent::DetectionStatus::Ambiguous
        );
        assert_eq!(codex.locations.len(), 2);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn path_root_count_and_size_limits_are_reported() {
        let path = (0..100)
            .map(|index| format!("/outside/{index}"))
            .collect::<Vec<_>>()
            .join(":");
        let plan = standard_search_plan(None, Some(std::ffi::OsStr::new(&path)), &[]);
        assert!(plan.initial_results.len() >= 64);
        assert!(plan.roots.len() <= 13);

        let oversized = "/opt/homebrew/bin/".repeat(2_000);
        let plan = standard_search_plan(None, Some(std::ffi::OsStr::new(&oversized)), &[]);
        assert!(
            plan.initial_results
                .iter()
                .any(|item| item.reason.contains("16384-byte"))
        );
    }

    #[test]
    fn custom_location_removal_excludes_it_from_later_search_plans() {
        let root = local_temp_root().join(format!("nexus-custom-remove-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let custom = validate_custom_location(&root).unwrap();
        let plan = standard_search_plan(None, None, std::slice::from_ref(&custom));
        assert!(plan.roots.iter().any(|item| item.directory == custom));
        let plan = standard_search_plan(None, None, &[]);
        assert!(!plan.roots.iter().any(|item| item.directory == custom));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ambiguous_names_remain_explicitly_ambiguous() {
        assert!(crate::agent::AgentId::Pi.basename_is_ambiguous());
        assert!(crate::agent::AgentId::Amp.basename_is_ambiguous());
        assert!(!crate::agent::AgentId::GeminiCli.basename_is_ambiguous());
    }

    #[test]
    fn cancellation_is_observed_before_filesystem_checks() {
        let plan = super::SearchPlan::default();
        let progress = Mutex::new(super::ScanProgress {
            scope: DiscoveryScope::Standard,
            current_location: None,
            locations_checked: 0,
            entries_examined: 0,
        });
        assert!(matches!(
            scan(
                &plan,
                DiscoveryScope::Standard,
                &AtomicBool::new(true),
                &progress
            ),
            ScanOutcome::Cancelled
        ));
    }
}
