use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);
pub const HISTORY_CAPACITY: usize = 300;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResourceSample {
    pub elapsed: Duration,
    pub rss_bytes: u64,
    pub cpu_percent: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollectionStatus {
    Starting,
    Running,
    Unavailable,
}

#[derive(Clone, Debug)]
pub struct TelemetrySnapshot {
    pub pid: u32,
    pub uptime: Duration,
    pub status: CollectionStatus,
    pub latest: Option<ResourceSample>,
    pub peak_rss_bytes: Option<u64>,
    pub history: VecDeque<ResourceSample>,
    pub last_error: Option<String>,
}

impl TelemetrySnapshot {
    fn new(pid: u32) -> Self {
        Self {
            pid,
            uptime: Duration::ZERO,
            status: CollectionStatus::Starting,
            latest: None,
            peak_rss_bytes: None,
            history: VecDeque::with_capacity(HISTORY_CAPACITY),
            last_error: None,
        }
    }

    fn record(&mut self, sample: ResourceSample, uptime: Duration) {
        self.uptime = uptime;
        self.status = CollectionStatus::Running;
        self.last_error = None;
        self.peak_rss_bytes = Some(
            self.peak_rss_bytes
                .map_or(sample.rss_bytes, |peak| peak.max(sample.rss_bytes)),
        );
        if self.history.len() == HISTORY_CAPACITY {
            self.history.pop_front();
        }
        self.history.push_back(sample);
        self.latest = Some(sample);
    }

    fn unavailable(&mut self, uptime: Duration, reason: String) {
        self.uptime = uptime;
        self.status = CollectionStatus::Unavailable;
        self.last_error = Some(reason);
    }
}

pub struct TelemetryService {
    state: Arc<Mutex<TelemetrySnapshot>>,
    stop: SyncSender<()>,
    worker: Option<JoinHandle<()>>,
}

impl TelemetryService {
    pub fn start() -> Self {
        let started_at = Instant::now();
        let pid = std::process::id();
        let state = Arc::new(Mutex::new(TelemetrySnapshot::new(pid)));
        let (stop, receiver) = mpsc::sync_channel(1);
        let worker_state = Arc::clone(&state);
        let worker = thread::Builder::new()
            .name("nexus-resource-monitor".to_owned())
            .spawn(move || collect_loop(receiver, worker_state, pid, started_at));

        match worker {
            Ok(worker) => Self {
                state,
                stop,
                worker: Some(worker),
            },
            Err(error) => {
                if let Ok(mut snapshot) = state.lock() {
                    snapshot
                        .unavailable(Duration::ZERO, format!("Could not start sampler: {error}"));
                }
                Self {
                    state,
                    stop,
                    worker: None,
                }
            }
        }
    }

    pub fn snapshot(&self) -> TelemetrySnapshot {
        self.state.lock().map_or_else(
            |_| {
                let mut unavailable = TelemetrySnapshot::new(std::process::id());
                unavailable.unavailable(
                    Duration::ZERO,
                    "Telemetry state lock is unavailable".to_owned(),
                );
                unavailable
            },
            |snapshot| snapshot.clone(),
        )
    }
}

impl Drop for TelemetryService {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn collect_loop(
    stop: Receiver<()>,
    state: Arc<Mutex<TelemetrySnapshot>>,
    pid: u32,
    started_at: Instant,
) {
    let sysinfo_pid = Pid::from_u32(pid);
    let mut system = match System::new() {
        Ok(system) => system,
        Err(error) => {
            mark_unavailable(&state, started_at, error.to_string());
            return;
        }
    };

    let mut cpu_samples = 0_u8;
    loop {
        if stop.try_recv().is_ok() {
            break;
        }

        let updated = system.refresh_processes_specifics(
            ProcessesToUpdate::Some(std::slice::from_ref(&sysinfo_pid)),
            true,
            ProcessRefreshKind::nothing()
                .with_memory()
                .with_cpu()
                .without_tasks(),
        );
        let elapsed = started_at.elapsed();

        match (updated, system.process(sysinfo_pid)) {
            (count, Some(process)) if count > 0 => {
                cpu_samples = cpu_samples.saturating_add(1);
                let sample = ResourceSample {
                    elapsed,
                    rss_bytes: process.memory(),
                    cpu_percent: (cpu_samples >= 2).then(|| process.cpu_usage()),
                };
                if let Ok(mut snapshot) = state.lock() {
                    snapshot.record(sample, elapsed);
                }
            }
            _ => mark_unavailable(
                &state,
                started_at,
                "NEXUS process metrics were not returned".to_owned(),
            ),
        }

        match stop.recv_timeout(SAMPLE_INTERVAL) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

fn mark_unavailable(state: &Mutex<TelemetrySnapshot>, started_at: Instant, reason: String) {
    if let Ok(mut snapshot) = state.lock() {
        snapshot.unavailable(started_at.elapsed(), reason);
    }
}

#[cfg(test)]
impl TelemetryService {
    /// A service that never spawns a sampling thread, for headless UI tests.
    pub(crate) fn inert() -> Self {
        let (stop, receiver) = mpsc::sync_channel(1);
        drop(receiver);
        Self {
            state: Arc::new(Mutex::new(TelemetrySnapshot::new(std::process::id()))),
            stop,
            worker: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CollectionStatus, HISTORY_CAPACITY, ResourceSample, TelemetryService, TelemetrySnapshot,
    };
    use std::time::Duration;

    fn sample(elapsed: u64, rss_bytes: u64) -> ResourceSample {
        ResourceSample {
            elapsed: Duration::from_secs(elapsed),
            rss_bytes,
            cpu_percent: Some(5.0),
        }
    }

    #[test]
    fn history_is_bounded_and_remains_in_chronological_order() {
        let mut snapshot = TelemetrySnapshot::new(42);
        for second in 0..(HISTORY_CAPACITY as u64 + 7) {
            snapshot.record(sample(second, second), Duration::from_secs(second));
        }

        assert_eq!(snapshot.history.len(), HISTORY_CAPACITY);
        assert_eq!(snapshot.history.front().unwrap().elapsed.as_secs(), 7);
        assert_eq!(snapshot.history.back().unwrap().elapsed.as_secs(), 306);
    }

    #[test]
    fn peak_is_kept_after_its_sample_leaves_the_history() {
        let mut snapshot = TelemetrySnapshot::new(42);
        snapshot.record(sample(0, 900), Duration::ZERO);
        for second in 1..=HISTORY_CAPACITY as u64 {
            snapshot.record(sample(second, 100), Duration::from_secs(second));
        }

        assert_eq!(snapshot.peak_rss_bytes, Some(900));
        assert_eq!(snapshot.history.len(), HISTORY_CAPACITY);
        assert_eq!(snapshot.history.front().unwrap().rss_bytes, 100);
    }

    #[test]
    fn unavailable_sample_preserves_last_values_and_marks_status() {
        let mut snapshot = TelemetrySnapshot::new(42);
        snapshot.record(sample(0, 700), Duration::ZERO);
        snapshot.unavailable(Duration::from_secs(2), "sample failed".to_owned());

        assert_eq!(snapshot.status, CollectionStatus::Unavailable);
        assert_eq!(snapshot.latest.unwrap().rss_bytes, 700);
        assert_eq!(snapshot.peak_rss_bytes, Some(700));
        assert_eq!(snapshot.last_error.as_deref(), Some("sample failed"));
    }

    #[test]
    fn service_samples_current_process_and_shuts_down_on_drop() {
        let service = TelemetryService::start();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);

        loop {
            let snapshot = service.snapshot();
            if let Some(sample) = snapshot.latest {
                assert_eq!(snapshot.pid, std::process::id());
                assert!(sample.rss_bytes > 0);
                assert!(snapshot.history.len() <= HISTORY_CAPACITY);
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "sampler did not produce a sample"
            );
            std::thread::sleep(Duration::from_millis(25));
        }

        drop(service);
    }
}
