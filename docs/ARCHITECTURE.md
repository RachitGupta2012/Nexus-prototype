# Architecture

## Principles

- Native Rust desktop application; one application process initially.
- Modular boundaries, but avoid creating a crate for every small feature.
- Local-first metadata and mission state; network access only through explicit integrations.
- Read-only discovery by default and explicit consent for execution or sensitive actions.
- Evidence-based status: show the last verified event when an agent cannot report live progress.

## Conceptual layers

1. **App/UI** — egui/eframe screens, navigation, user approvals, Mission Control.
2. **Core domain** — typed entities, events, errors, task and mission state machines.
3. **Discovery** — bounded inventory of known apps, executables, processes, and supported configuration metadata.
4. **Integration layer** — adapters for ACP, MCP, documented CLIs, and direct provider APIs.
5. **Registry** — agents, providers, models, capabilities, authentication state, and evidence provenance.
6. **Intelligence/router** — compatibility filtering and task-specific candidate scoring.
7. **Mission engine** — decomposition, delegation, budgets, cancellation, checkpoints, review, and artifact aggregation.
8. **Monitoring** — NEXUS process metrics and child-process metrics kept separate.
9. **Storage** — SQLite persistence, bounded event history, cache expiry.

## Entity distinctions

- Agent harness: the system that manages an agent session and tools.
- Provider/gateway: service that serves model inference or routes requests.
- Model: a specific model identity or provider-specific variant.
- Adapter: a supported bridge from NEXUS to an external system.

Do not conflate these. For example, a coding harness is not itself a model provider.

## Suggested initial workspace

Start with a small number of crates or modules and split only when boundaries are justified:

- `nexus-app`: native UI and application entry point.
- `nexus-core`: domain types, configuration, events, and errors.
- `nexus-host`: process and OS integration abstractions.
- `nexus-integrations`: discovery and agent/provider adapters.
- `nexus-engine`: missions, routing, and orchestration.
- `nexus-storage`: SQLite persistence.

The exact split is for Codex to validate during Phase 0. Do not create empty crates merely to match this list.

## Runtime model

Initially run as one desktop application process. Use Tokio for bounded asynchronous work. Do not add a daemon, localhost server, or IPC layer unless a concrete requirement demonstrates the need.

## Evidence and status

Every externally sourced benchmark or capability claim should record source, retrieval time, and confidence/freshness. If an agent does not expose progress, display the last verified event rather than inventing activity.

## Phase 1 process telemetry

The initial Resource Monitor samples only NEXUS's own PID through a process-specific provider. It keeps a bounded ten-minute in-memory history and does not inspect unrelated processes. Future child-agent metrics must remain a separate stream and must never be included in NEXUS's own process totals.
