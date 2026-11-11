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

## Phase 2 discovery and registry

Discovery is an explicit user action. Standard Discovery uses a short-lived standard-library worker to check exact candidate names in at most 64 eligible PATH roots (16 KiB input), conventional macOS package-manager/user binary roots, and optional user-approved direct custom directories. The full roots and skip reasons are displayed. Expanded Discovery requires explicit scope consent and walks only `/Applications`, `/opt`, and the current user's `~/Applications` to depth 4, at most 128 directories/3,000 entries, or 10 seconds. It skips symlinked directories and protected/personal-data subtrees. Both modes are cancelable; cancellation is checked between filesystem operations. Custom folders use a native macOS folder picker and are rejected when path components are symlinks or the filesystem is remote/unrecognized. Safe platform filesystem metadata comes through the small `nix` wrapper; folder picking uses macOS AppKit through `rfd`. No shell, package manager, candidate, process list, credentials, configuration, or network is accessed.

`DiscoveryScope` remains separate from `IntegrationStatus`. Scope consent authorizes only passive local detection. The approval version is persisted so any material catalog/scope change can require renewed consent. Custom paths are stored only in NEXUS's registry and authorize exact nonrecursive checks directly in the selected directory. Registry search-result details are bounded and disclose when the presentation limit is reached.

The registry stores NEXUS-owned metadata in `registry.toml` beside the application configuration. Writes use a same-directory temporary file, restrictive permissions, sync, and atomic rename after validating the destination and rejecting symlink components. A scan updates only this NEXUS-owned file. It does not alter agent files. The initial registry contains candidates and evidence only; provider/model catalogs and adapters remain separate future work.

Candidate detection, version, running state, authentication, connectability, and usability remain distinct. Vendor documentation is recorded as documentation evidence, not proof about the detected installation. Version verification is a separate user-run operation; NEXUS does not execute version commands during discovery. No adapter is implemented in Phase 2.

## Phase 3 task routing

Routing is a synchronous, pure evaluation over a validated in-memory `Task`, a read-only snapshot of `AgentRecord`s, a curated Rust capability catalog, and an explicit evaluation timestamp. `task.rs` owns bounded task input and manual categories; `capability.rs` owns stable capability identifiers and provenance-bearing evidence; `routing.rs` compares the inputs and returns explanations and outcomes. It has no filesystem, process, network, credential, or registry-write access. The UI retains task text and a manual selection only in the current session.

The initial catalog documents software-development workflow claims for the existing nine candidates. It intentionally leaves individual file operations, research, data analysis, document processing, writing, structured output, and tool usage unknown. Discovery never implies a capability. Evidence is version-matched only when an independently observed installed version exactly matches the claim. Documentation without a version is version-unverified; claims older than 365 days, undated, future-dated, older-version, or conflicting evidence are not usable to establish eligibility. No verified capability is asserted by the initial catalog, so the initial product can produce tentative software-development recommendations, while mandatory unlisted capabilities produce insufficient-evidence outcomes.

Outcomes are Supported, Tentative, Insufficient evidence, Rejected, or Excluded. Ranking is a documented lexicographic comparison of outcome, evidence quality, category evidence, and matched optional preferences. Exact semantic ties remain tied; `AgentId` order only stabilizes presentation. Phase 3 is recommendation-only. Authorization remains false and no task is persisted.

## macOS application detection

Application bundles have a separate typed detection record rather than being represented as CLI executables. Each explicit scan checks the two curated bundle names directly in `/Applications` and `~/Applications`; no directory traversal or bundle-content read is performed for this check. Results are persisted in the NEXUS registry with a serde default for compatibility. Application detection does not make a bundle eligible for the CLI routing engine.

## Explicit provider connection

`provider.rs` implements a narrow direct provider client for OpenAI-compatible Chat Completions. It is separate from the pure `routing.rs` recommendation engine and from the discovered agent registry. A compatible local gateway (including a FreeLLMAPI-style gateway) can provide model selection/routing by using its documented base URL and model selector; NEXUS does not claim to support every vendor API protocol.

Non-secret endpoint and model settings live in `config.toml`. On macOS, a user-entered provider key is stored through the OS Keychain and is not serialized or logged. Keychain reads happen only on the worker spawned by an explicit **Send task to provider** action. That operation sends the user's task description to the configured endpoint and displays a text response. It never launches a discovered agent, changes files, or authorizes execution. Requests use HTTPS for remote endpoints, allow plain HTTP only for loopback, do not follow redirects, have a total timeout, and cap the response body. The UI thread remains responsive while the bounded request is in flight.
