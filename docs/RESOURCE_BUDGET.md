# Resource budget

The NEXUS application process must remain below 1 GB RAM.

## Initial engineering targets (not yet achieved)

| State | NEXUS process target |
|---|---:|
| Idle | below 250 MB |
| Typical mission | below 500 MB |
| Heavy workload | below 750 MB |
| Warning | 700 MB |
| Critical internal threshold | 900 MB |
| Absolute requirement | below 1 GB |

## Measurement rules

- Record the metric name and source (RSS, physical footprint, or other).
- Do not compare different memory metrics as if they were identical.
- Report NEXUS separately from child agents and external applications.
- Show aggregate child-process usage separately when available.
- Cloud inference resource usage is unknown unless the provider reports it; do not invent it.

## Engineering practices

- No local LLM runtime in the initial product.
- No mandatory Docker, Python, or Node service.
- Avoid continuous full-disk scans and unnecessary polling.
- Use bounded channels, bounded logs, cache expiry, and on-demand adapters.
- Profile release builds on the target Mac.
- Add regression tests or repeatable profiling scenarios as the application grows.

Rust alone does not guarantee low memory use. Measure real builds and workloads.

## Phase 1 telemetry

The Resource Monitor samples only the NEXUS process. RSS is reported in decimal MB to match the thresholds above; it is not interchangeable with macOS physical footprint. CPU is process CPU percentage relative to one logical core and may exceed 100% when multiple cores are used. Uptime and peak observed RSS are session-local.

Sampling runs every two seconds and retains at most 300 samples (ten minutes) in memory. No resource history is written to disk or transmitted. If a sample fails, the last successful value remains visible as stale and the app continues running. The telemetry worker is stopped and joined when the application exits.

For an enabled/disabled comparison, build the default release app with telemetry enabled and sample its own PID using `ps -p PID -o pid,rss,%cpu,etime,command`. Then build and run with `cargo build --release --no-default-features`; this build retains the Resource Monitor section with collection disabled. Use the same Mac, display setup, idle period, and sampling window for both. Record RSS and CPU as reported by `ps`, and do not label RSS as physical footprint. The earlier 87.5 MiB reading remains a single Phase 0 snapshot, not a sustained baseline.

Phase 2 discovery is on demand and checks only exact candidate names in bounded local roots on a short-lived worker thread. Standard roots are nonrecursive; user custom roots are direct checks only. Expanded scope is explicit and capped at 128 directories, 3,000 entries, depth 4, and 10 seconds. Registry state remains capped at 64 KiB on disk; scan detail history is bounded to 256 location results and says when details are omitted. Discovery adds no polling or monitoring service.

The native folder picker (`rfd`) and safe macOS filesystem metadata wrapper (`nix`, `fs` feature only) add compiled dependencies, but no background runtime. Runtime RSS/CPU overhead of those dependencies and of an expanded scan has not been measured as a sustained benchmark. Expected transient discovery overhead is small but is an estimate only; measure the NEXUS PID before, during, and after a scan on the target Apple Silicon Mac. Do not include the picker dialog's user time as active scanner work. The dependency graph also includes the macOS Objective-C binding support used by the picker.
