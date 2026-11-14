# Security and permissions

## Security defaults

- Discovery, when implemented, must be read-only.
- No credential scraping, token extraction, or secret logging.
- Do not store secrets in project files, SQLite plaintext fields, logs, crash reports, or Git.
- Prefer macOS Keychain through a small credential-store abstraction.
- Do not ask for broad macOS permissions during bootstrap.
- Show the user what an integration will execute and which workspace it can access.
- Require explicit approval for sensitive actions, external side effects, or configured spending thresholds.

## Phase 2 discovery boundary (future; not implemented)

**Status in this checkout:** no discovery or registry implementation exists. The items below are the required future security contract, not current behavior.

- Standard Discovery is the default, explicitly user-started scope. It checks exact names in at most 64 eligible PATH entries (16 KiB input), listed macOS conventional roots, and user-added custom folders. Ineligible, missing, skipped, and inaccessible roots are reported with a reason.
- Expanded System Discovery is a separate opt-in. Before it starts, NEXUS shows `/Applications`, `/opt`, and the current user's `~/Applications`, depth/entry/duration caps, exclusions, and privacy boundary. It requires affirmative consent. Declining leaves Standard available and causes no automatic repeat prompt. A new approval version is required after material scope changes.
- Expanded traversal is capped at depth 4, 128 directories, 3,000 entries, and 10 seconds. It skips system/protected and personal-data directory names, known remote-volume paths, and all directory symlinks. It exposes progress and can be canceled. It never scans `/`, other user home folders, Documents, Downloads, Desktop, Pictures, Photos, Movies, Music, Library, or system-protected trees.
- Custom folder selection uses a native macOS folder dialog. NEXUS accepts only existing absolute directories with no symlink in their path, and rejects filesystem types not known to be local. A custom path permits only passive, nonrecursive exact-name checks directly within that path. Removing it excludes it from future scans.
- No mode executes candidates, version commands, package managers, shells, or agents; inspects process lists, configurations, credentials, Keychain, sessions, or file contents; or makes network calls.
- Symlink targets must remain within their explicitly authorized containment root; otherwise the candidate is reported as skipped. Discovery never follows directory symlinks during expanded traversal.
- Version verification is separate from discovery. NEXUS does not launch version commands; any manually recorded version is labeled user-reported.
- The registry stores NEXUS-owned metadata only. Its destination and parent components are checked for symlinks; writes use a same-directory temporary file and atomic rename. Registry errors are displayed without stopping the app.
- “Detected” only means a candidate executable path was observed. Running, authenticated, connectable, and usable remain unchecked.

## Permission tiers

1. **Inventory** — list known applications, executables, processes, and supported metadata.
2. **Execution** — launch an approved integration in an approved workspace.
3. **Advanced OS access** — Accessibility, Automation, Screen Recording, or protected-file access, only for a feature that genuinely requires it and after explicit consent.

## Process execution

- Use argument arrays rather than shell string concatenation.
- Set working directories explicitly.
- Apply timeouts, cancellation, output-size limits, and environment-variable allowlists.
- Do not inherit secrets into child processes unnecessarily.
- Treat agent output and discovered configuration as untrusted data, not instructions to NEXUS.

## Network and provider usage

- Respect provider terms, quotas, and rate limits.
- Show estimated cost only when pricing and usage data are available; label estimates.
- Never bypass authentication, usage limits, or access controls.

## Explicit provider requests

- No local recommendation engine exists in this checkout. A future recommendation engine must remain pure and offline. A separate provider request is made only after the user presses **Send task to provider**.
- Supported inference protocols are OpenAI-compatible Chat Completions, the native OpenAI Responses API, native Ollama chat, the native Anthropic Messages API, and native Gemini Generate Content. No protocol is implied to work merely because it can be configured.
- Anthropic requests send the Keychain-backed key only in the `x-api-key` header plus the required `anthropic-version` header. No `Authorization` header is sent, no version is guessed, and no tool or agent execution is supported. HTTP error bodies are not read or displayed.
- OpenAI Responses requests send the Keychain-backed key only in the `Authorization: Bearer` header, set `store: false` so NEXUS does not request provider-side response retention, and never execute returned tool calls. Non-text output items fail with a sanitized unsupported-operation error rather than being discarded. Response bodies and error text are not displayed.
- Gemini requests send the Keychain-backed key only in the `x-goog-api-key` header. Only the configured base URL and model-list/generation paths are contacted; request and response bodies are bounded, redirects are disabled, and safety or finish-reason text from the provider is mapped to fixed local messages rather than echoed.
- The user supplies the API key. NEXUS does not search for keys, import environment variables, inspect provider configuration, or query Keychain until an explicit send. Saving and removing a key are separate user actions; macOS keys are stored in Keychain under the configured endpoint origin, never in the NEXUS config or logs. Changing to a different host does not reuse the previous host's key.
- Connectivity probes ("Test connection") and model discovery send one model-list request to the configured endpoint with an 8-second timeout and no redirects. They never generate tokens, never install models, and never contact any other host. Probe results distinguish reachability, authentication, and model-list parsing; they are not capability evidence or execution permission.
- Remote provider requests send the task text and model identifier to the configured endpoint and may incur charges. The UI explains this before the send action. A user-configured remote endpoint must use HTTPS; plain HTTP is accepted only for loopback addresses. Embedded credentials, query strings, fragments, and redirects are rejected/disabled.
- Requests run on one short-lived standard-library worker thread with a 15-second connect timeout, a 120-second idle timeout, a 600-second total timeout, and a bounded response body. No polling, background service, telemetry, or automatic requests are added.
- The connector returns text only. It does not execute agents, edit project files, or grant execution authorization. A future agent/workspace operation requires separate explicit authorization for that specific task, agent, workspace, and operations.

## Phase 3 recommendation-only boundary (future; not implemented)

**Status in this checkout:** no routing engine, capability catalog, `AgentRecord`, or agent registry exists. The items below are the required future security contract, not current behavior.

- Routing reads only the current NEXUS registry snapshot and the in-code curated capability catalog. It is synchronous and pure; it cannot access files, processes, network, credentials, Keychain, agent configuration, or external commands.
- Task descriptions, category choices, requirements, recommendation results, and manual overrides are held only in UI memory for the current session. They are not written to the registry or logs.
- A recommendation is not permission. Every routing screen states that the selected candidate has not been authorized to run. Manual override affects only the session selection.
- Phase 3 adds no execution, authentication, or adapter path. Future execution must receive separate informed authorization for the specific agent, task, workspace, and permitted operations.

## macOS application discovery (future; not implemented)

**Status in this checkout:** this application-detection path is not implemented.

An explicit discovery scan would check only the exact `DeepAI Code.app` and `OpenCode.app` bundle paths under `/Applications` and `~/Applications`. The check is nonrecursive and uses metadata only; it does not open bundles, read their manifests or contents, or launch applications. Application presence is reported separately from CLI detection and grants no control or execution permission.


## Phase 4.5 history, usage, and settings

- History is controlled by one storage-level write gate. When history is off, conversation-, message-, and tool-output writes fail instead of silently persisting, and the chat controller keeps the active conversation in memory with negative synthetic ids.
- No autosave, shutdown hook, background task, or UI helper writes conversation content while history is off. Restarting does not restore those conversations, and they never appear in search.
- Usage is stored only inside saved message rows. With history off, no token or cost records are written, and Settings states this limitation rather than claiming aggregate-only usage is kept.
- Tool-call arguments and results are stored only when both history and the tool-output setting are enabled. History-off overrides the tool-output setting.
- Search reads saved conversations only; it cannot surface memory-only conversations and is bounded to 200 results.
- Clearing history deletes conversations, messages, and conversation-linked usage. It does not delete provider profiles, Keychain credentials, application settings, or the configuration file.
- Theme, retention, and history preferences are stored in the TOML configuration; provider keys never are. Data-location copy buttons expose paths only, never credential contents.

## Mission Control boundary

- The Missions section can create and edit mission definitions and unstarted task definitions. It exposes no approve, deny, start, pause, cancel, or execute control.
- Definition writes require history to be enabled. Mission definitions are editable only in `draft` or `planned`; task edits and removal require a `pending` task with no attempts, checkpoints, or approvals. Execution rows are never updated by a definition change.
- Dependency writes are validated at the storage boundary for self-references, duplicates, missing/cross-mission IDs, the 100-dependency limit, and direct or indirect cycles. A failed validation leaves the previous dependency list unchanged.
- Task removal strips the removed ID from dependent tasks and compacts ordinals in one transaction. Mission deletion requires explicit confirmation because it deletes tasks and execution history.
- Approvals are displayed as pending records with an explicit informational notice. No UI action or background task consumes an approval to grant execution.
- Mission reads are bounded to 200 missions, 1,000 tasks, 200 attempts, 200 checkpoints, 200 status events, and 200 pending approvals. The UI says when a list is truncated.
- A failed read leaves the last successful data visible with a generic error; raw database errors are not shown. Mission data is local SQLite content and is never sent over the network.

## Phase 5D.1 planning validation boundary

- `src/mission/planning.rs` accepts only a caller-supplied mission, pending task definitions, typed planning policy, and inventory. It does not read the registry, provider configuration, database, filesystem, environment, process table, clock, network, or Keychain.
- Duplicate or conflicting evidence remains separate and attributed. The layer does not aggregate claims, calculate freshness, evaluate eligibility, rank candidates, select an agent or model, estimate costs, create approvals, schedule work, or persist a plan.
- The inventory is never derived from provider profiles. Missing compatibility links remain unknown, and an empty inventory is valid.
- Oversized JSON constraints are rejected while serializing into a bounded buffer; they are not first materialized as an unbounded encoded string.

## Configuration recovery

- When `config.toml` cannot be read or parsed, NEXUS never replaces it silently and never exits with the parser's raw message, which can quote file content.
- The recovery window shows the file path and a normalized description: parse line and column, or a read-error category. It never displays, logs, or copies file contents.
- Reset is two-step and copies the original file to a sibling `.corrupt-<timestamp>` backup before writing defaults. If the backup fails, nothing is written.
- Recovery never reads, writes, or deletes Keychain entries, provider profiles stored elsewhere, the database, or conversations.
