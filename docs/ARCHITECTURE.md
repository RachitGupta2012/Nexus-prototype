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

**Status in this checkout:** the agent-discovery and registry implementation described below is not present in the source tree. Provider profiles are not a substitute for that registry. The following is the required Phase 2 contract and remains a prerequisite for real agent routing.

The required implementation would make discovery an explicit user action. Standard Discovery would use a short-lived standard-library worker to check exact candidate names in at most 64 eligible PATH roots (16 KiB input), conventional macOS package-manager/user binary roots, and optional user-approved direct custom directories. The full roots and skip reasons are displayed. Expanded Discovery requires explicit scope consent and walks only `/Applications`, `/opt`, and the current user's `~/Applications` to depth 4, at most 128 directories/3,000 entries, or 10 seconds. It skips symlinked directories and protected/personal-data subtrees. Both modes are cancelable; cancellation is checked between filesystem operations. Custom folders use a native macOS folder picker and are rejected when path components are symlinks or the filesystem is remote/unrecognized. Safe platform filesystem metadata comes through the small `nix` wrapper; folder picking uses macOS AppKit through `rfd`. No shell, package manager, candidate, process list, credentials, configuration, or network is accessed.

`DiscoveryScope` remains separate from `IntegrationStatus`. Scope consent authorizes only passive local detection. The approval version is persisted so any material catalog/scope change can require renewed consent. Custom paths are stored only in NEXUS's registry and authorize exact nonrecursive checks directly in the selected directory. Registry search-result details are bounded and disclose when the presentation limit is reached.

The registry stores NEXUS-owned metadata in `registry.toml` beside the application configuration. Writes use a same-directory temporary file, restrictive permissions, sync, and atomic rename after validating the destination and rejecting symlink components. A scan updates only this NEXUS-owned file. It does not alter agent files. The initial registry contains candidates and evidence only; provider/model catalogs and adapters remain separate future work.

Candidate detection, version, running state, authentication, connectability, and usability remain distinct. Vendor documentation is recorded as documentation evidence, not proof about the detected installation. Version verification is a separate user-run operation; NEXUS does not execute version commands during discovery. No adapter is implemented in Phase 2.

## Phase 3 task routing

**Status in this checkout:** the routing and agent-registry implementation described below is not present. There is no `task.rs`, `capability.rs`, `routing.rs`, `AgentRecord`, or agent registry. The provider configuration in `config.toml` is not an agent inventory and cannot stand in for one. The following is the required Phase 3 contract; it must be restored or implemented before NEXUS can make real agent or model recommendations.

Routing is a synchronous, pure evaluation over a validated in-memory `Task`, a read-only snapshot of `AgentRecord`s, a curated Rust capability catalog, and an explicit evaluation timestamp. `task.rs` owns bounded task input and manual categories; `capability.rs` owns stable capability identifiers and provenance-bearing evidence; `routing.rs` compares the inputs and returns explanations and outcomes. It has no filesystem, process, network, credential, or registry-write access. The UI retains task text and a manual selection only in the current session.

The initial catalog documents software-development workflow claims for the existing nine candidates. It intentionally leaves individual file operations, research, data analysis, document processing, writing, structured output, and tool usage unknown. Discovery never implies a capability. Evidence is version-matched only when an independently observed installed version exactly matches the claim. Documentation without a version is version-unverified; claims older than 365 days, undated, future-dated, older-version, or conflicting evidence are not usable to establish eligibility. No verified capability is asserted by the initial catalog, so the initial product can produce tentative software-development recommendations, while mandatory unlisted capabilities produce insufficient-evidence outcomes.

Outcomes are Supported, Tentative, Insufficient evidence, Rejected, or Excluded. Ranking is a documented lexicographic comparison of outcome, evidence quality, category evidence, and matched optional preferences. Exact semantic ties remain tied; `AgentId` order only stabilizes presentation. Phase 3 is recommendation-only. Authorization remains false and no task is persisted.

## macOS application detection

Application bundles have a separate typed detection record rather than being represented as CLI executables. Each explicit scan checks the two curated bundle names directly in `/Applications` and `~/Applications`; no directory traversal or bundle-content read is performed for this check. Results are persisted in the NEXUS registry with a serde default for compatibility. Application detection does not make a bundle eligible for the CLI routing engine.

## Explicit provider connection

`provider.rs` implements a narrow direct provider client for OpenAI-compatible Chat Completions. It is separate from the future pure `routing.rs` recommendation engine and from the future discovered agent registry. A compatible local gateway (including a FreeLLMAPI-style gateway) can provide model selection/routing by using its documented base URL and model selector; NEXUS does not claim to support every vendor API protocol.

Non-secret endpoint and model settings live in `config.toml`. On macOS, a user-entered provider key is stored through the OS Keychain and is not serialized or logged. Keychain reads happen only on the worker spawned by an explicit **Send task to provider** action. That operation sends the user's task description to the configured endpoint and displays a text response. It never launches a discovered agent, changes files, or authorizes execution. Requests use HTTPS for remote endpoints, allow plain HTTP only for loopback, do not follow redirects, have a total timeout, and cap the response body. The UI thread remains responsive while the bounded request is in flight.


## Phase 5 mission persistence foundation

`src/mission.rs` defines the durable mission model: missions, ordered tasks, task attempts, checkpoints, and approval requests. Each status has an explicit string encoding and a closed transition table, so persistence rejects illegal jumps such as `completed → running` instead of letting a future engine corrupt history.

SQLite schema version 4 adds `missions`, `mission_tasks`, `task_attempts`, `checkpoints`, and `approvals`, then adds mission deliverables, task dependencies, per-task status events, and a typed mission privacy policy. Each upgrade is additive: earlier databases keep existing rows and receive only the new columns and tables. Existing missions default to `local_only` privacy. Foreign keys cascade from a mission to tasks, attempts, checkpoints, and approvals; a deleted task nulls checkpoint/approval task references so the mission-level record survives.

Deliverables and dependencies are validated before use. Dependencies must name tasks in the same mission, cannot repeat, cannot reference the task itself, and are limited to 100 per task. A shared graph check rejects direct and indirect cycles before either the UI or storage writes them. Every task receives an initial `pending` status event, and later status changes append an event so history cannot be silently rewritten. Status history reads are clamped to 200 events.

All mission writes pass through the history-write gate. With history disabled, creating or mutating missions fails while reads and deletion remain available, matching the conversation boundary. Clearing history deletes missions as well as conversations.

Inputs are bounded before storage: 200-byte titles, 16 KiB objectives, 4 KiB workspaces, 65 KiB JSON objects, 1,000 tasks per mission, 200 attempts per task, 500 checkpoints and 500 approvals per mission. Read APIs clamp to 200 missions, 1,000 tasks, and 200 attempts/checkpoints/approvals. Task attempts and approval decisions are append-only in practice: a finished attempt cannot be finished again, and a decided approval cannot be re-decided.

`src/ui/missions.rs` provides Mission Control. It creates and edits mission definitions (title, objective, workspace, privacy, JSON constraints, and deliverables) and task definitions, edits dependencies, reorders tasks, and removes unstarted tasks. Definition fields are validated before saving and again at the storage boundary. Task removal is transactional: dependencies on the removed task are stripped and remaining ordinals are compacted.

Definition editing is deliberately separate from execution history. Mission definitions are editable only in `draft` or `planned`; task definitions are editable only while the task is `pending` and has no attempts, checkpoints, or approvals. Update methods never touch task status, timestamps, attempts, checkpoints, approvals, or status events. Mission deletion is the only operation that deliberately removes execution history, and the UI requires confirmation.

The UI shows a bounded mission overview with creation/update information, mission detail with constraints, deliverables, ordered tasks, dependencies, privacy, task detail with attempts, checkpoints, and status history, plus a separate pending approval queue labelled informational. Selection loads on click or explicit refresh; no database read occurs per frame, and all lists state when they are truncated. Empty, deleted-mission, and load-error states are explicit, and the last successful data stays visible after a read failure.

This layer performs no decomposition, scheduling, scoring, provider calls, tool calls, status transitions, or approvals. Approvals are records only; the Mission Control UI has no approve or deny control, and no code consumes an approved record to grant execution.

### Planning-domain foundation (implemented)

`src/mission/planning.rs` defines the Phase 5D.1 planning input boundary. It is a pure, local validation layer: it accepts a draft or planned mission, its pending task definitions, typed budget/requirements/preferences, and a caller-supplied inventory of agents, models, and sparse agent-model compatibility links.

Each compatibility and capability record keeps its own evidence claim (`supported`, `unsupported`, or `unknown`) and provenance. Locality records separately keep `local`, `remote`, or `unknown`. Missing links mean unknown compatibility. The planner does not aggregate conflicts, calculate freshness, filter, rank, select, estimate costs, create approvals, or persist a plan. It reads no clock, filesystem, network, credentials, database, or registry.

The inventory is not derived from provider profiles. An empty inventory is valid. No real agent or model recommendation is possible until the Phase 2 registry and Phase 3 evidence-producing router exist.

Validation requires positive mission and task IDs and is bounded to 64 agents, 2,000 models, 4,096 explicit links, 32 capabilities per agent, eight evidence records per capability or link, two locality records per candidate, 1,000 tasks, 12 global requirements, 128 submitted requirement entries before deduplication, 128 task requirement groups, 12 entries per preference class, 24 total preferences, and 100 dependencies per task. Task dependencies reuse the shared mission validators, and dependency lists are validated before any owned edge list is built. Oversized mission JSON constraints are rejected through bounded serialization.

A model candidate has one currency field shared by its input and output prices. Phase 5D.1 therefore cannot represent two independently priced currencies on one model; no currency conversion or cost calculation is performed. Normalization is deterministic: agents, models, links, capabilities, evidence, requirements, and preferences are sorted by stable keys without discarding conflicting evidence, and equivalent shuffled inputs produce the same normalized output.

## MCP read-only client foundation

`src/mcp/` owns the MCP boundary. `config.rs` validates persisted server settings without I/O; `manager.rs` owns one background OS thread per explicit connection and a bounded event channel; the UI only polls that channel. `rmcp` 3.10 is compiled with `client`, `transport-child-process`, and `transport-streamable-http-client-reqwest` only, so no server implementation, sampling, or OAuth machinery is linked.

The client negotiates through `rmcp`, including the stateless 2026-07-28 revision, and displays the negotiated version, server identity, and advertised capabilities. Discovery is a bounded sequence of `tools/list` pages. Resource and prompt capabilities are reported but never exercised. There is deliberately no `tools/call`, resource read, prompt retrieval, sampling, elicitation, auto-connect, or auto-reconnect path; a source test fails if those method names appear in `src/mcp/`.

STDIO commands are spawned directly, never through a shell. Environment variables are cleared and only names in the server's allowlist are copied, plus `NEXUS_MCP_CLIENT=1`. stderr is drained with a 64 KiB cap and never shown as protocol content. Streamable HTTP uses a dedicated `reqwest` client with redirects disabled, a 120-second idle timeout, and an 8 MiB SSE event cap; URLs must use HTTPS or loopback HTTP and may not embed credentials.

Metadata is untrusted: at most 1,000 tools, 10 pages, 256-byte names, 8 KiB descriptions, and 256 KiB per schema are retained, duplicate names are skipped, and errors are mapped to fixed local messages rather than raw SDK text. Disconnect closes the transport with a 3-second timeout. A retained catalog is marked stale after disconnect or a failed refresh. No tool execution or agent loop consumes this catalog in this milestone.

## Phase 4.5 UI and history boundary

The desktop shell routes six sections: Chat, Providers, MCP, Usage, Resources, and Settings. Settings owns appearance and history preferences; Providers owns profiles, model catalogs, prices, and Keychain actions; Usage reads bounded aggregates from SQLite.

Persistence has one boundary. `Storage` carries a history-write gate and a tool-output gate. Content writes return `HistoryDisabled` when the gate is closed, while reads, deletion, and pruning stay available. `ChatController` checks the same policy so chat keeps working without a database row, using negative conversation and message ids to mark memory-only state. This avoids a second storage implementation and keeps UI conditionals from being the only enforcement point.

Usage is derived from saved assistant messages rather than a separate aggregate table. That keeps history-off truly contentless but also means no usage dashboard data exists while history is off. Search is a bounded `LIKE` query over titles and message bodies; there is no full-text index and results are capped at 200.
