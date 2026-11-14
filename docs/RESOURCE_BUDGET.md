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

## Phase 2 discovery overhead (future; not implemented)

**Status in this checkout:** no discovery or registry implementation exists, and `Cargo.toml` does not include `rfd` or `nix`. The following are future design expectations, not measured current behavior.

A future on-demand discovery implementation would check only exact candidate names in bounded local roots on a short-lived worker thread. Standard roots would be nonrecursive; user custom roots would be direct checks only. Expanded scope would be explicit and capped at 128 directories, 3,000 entries, depth 4, and 10 seconds. Registry state would be capped at 64 KiB on disk; scan detail history would be bounded to 256 location results and would say when details are omitted. Discovery would add no polling or monitoring service.

A future native folder picker and macOS filesystem metadata wrapper would add compiled dependencies, but no background runtime. Runtime RSS/CPU overhead of those dependencies and of an expanded scan has not been measured. Any expectation of small transient overhead is an estimate only; it is not a current measurement or a compliance claim.

## Phase 3 routing overhead (future; not implemented)

**Status in this checkout:** no routing engine, registry candidate set, or **Recommend** action exists. The following describes a future bounded design, not current behavior.

A future task input would be limited to 16 KiB plus at most 12 global requirements, task-specific entries under the total requirement cap, and 24 total preferences. Recommendation results and catalog evidence would be small bounded vectors derived from a bounded registry snapshot. A future evaluator would be synchronous and would add no thread, polling, persistence, network request, or dependency. Its memory and CPU use has not been benchmarked; existing process targets remain targets rather than measured guarantees.

## Phase 5D.1 planning validation

Planning validation is synchronous and in-memory. Inputs are bounded to 64 agents, 2,000 models, 4,096 links, 1,000 tasks, 12 global requirements, 128 submitted requirement entries, 128 task requirement groups, 24 preferences, 100 dependencies per task, eight evidence records per capability or link, and two locality records per candidate. Oversized mission JSON constraints are rejected while writing into a bounded serialization buffer rather than after allocating the complete encoded value.

No separate RSS or CPU benchmark has been run for planning validation. The current claim is limited to bounded input and allocation-safe validation; it is not a measured performance guarantee.

## Explicit provider request overhead

The provider client creates one standard-library worker thread only after an explicit send, then exits after a response or error. The task prompt is capped by the existing 16 KiB task limit and response bodies at 8 MiB; one in-flight request is allowed. The HTTP client has a 15-second connect timeout, a 120-second idle timeout, and a 600-second total timeout. The UI polls its bounded result channel while the request is active. The request and response buffers, TLS state, and dependency code add transient memory and CPU use. This overhead has not been measured; no resource improvement or target compliance is claimed. Provider-side/cloud usage is not NEXUS process resource usage.
