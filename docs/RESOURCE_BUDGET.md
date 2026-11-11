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
