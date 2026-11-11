# NEXUS

NEXUS is a native Rust desktop application for discovering, evaluating, and orchestrating AI agents, model providers, and tools available on a user's computer.

**Current status:** planning / repository bootstrap. The implementation must proceed in small, testable phases.

## Initial goals

- Discover supported AI agent harnesses, model providers, CLIs, MCP servers, and ACP agents.
- Keep agent harnesses, providers, and individual models as separate concepts.
- Recommend and coordinate agents/models for a task using compatibility, task-specific quality evidence, cost, latency, availability, and observed outcomes.
- Provide a native Mission Control interface for task status and resource monitoring.
- Respect user permissions, provider terms, and documented integration interfaces.

## Non-goals for the initial prototype

- No local LLM runtime.
- No Docker, Python backend, Node backend, Electron, Tauri, or web frontend.
- No reverse engineering private APIs or bypassing provider quotas.
- No automatic credential extraction.
- No claim of live progress or cloud resource usage unless an integration actually reports it.

## Initial platform

- macOS on Apple Silicon.
- Rust workspace.
- Native GUI: `egui` / `eframe`.
- Async runtime: Tokio.
- Persistence: SQLite.
- System monitoring: `sysinfo`, with native macOS measurements added only where justified.

## Getting started

Read `AGENTS.md` and the documents in `docs/` before implementing anything.

The first task is `docs/CODEX_FIRST_TASK.md`. Implement Phase 0 only; do not jump ahead to agent integrations.

## Resource objective

NEXUS's own process should remain below 1 GB RAM. Initial engineering targets:

- Idle target: below 250 MB.
- Typical mission target: below 500 MB.
- Heavy workload target: below 750 MB.
- Warning threshold: 700 MB.
- Critical internal threshold: 900 MB.

Measure actual release-build memory. Do not treat these targets as already achieved. Track NEXUS separately from child agent processes.
