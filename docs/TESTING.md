# Testing

## Automated checks

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release
```

`cargo test` includes headless UI tests and PNG snapshot comparisons.

## UI interaction tests

`egui_kittest` (dev-dependency only, same 0.37 release as egui/eframe) drives the real `NexusApp` through AccessKit: tests click by widget label, type into text fields, and assert on application state. No production dependency or runtime code changes are involved.

Covered interactions (`src/ui/tests.rs`):

- navigation across Chat, Providers, MCP, Usage, and Settings
- theme selection written to the configuration file
- history toggle updating the storage write gate and the file
- conversation search filtering and the no-match state
- opening a saved conversation
- delete and clear-all confirmation flows, including cancel
- empty states for conversations and usage
- first-run callout condition and persisted dismissal
- accessibility-name checks for every screen, including saved-conversation rows,
  provider profile fields, API-key inputs, connectivity buttons, and every model
  catalog field (ID, display name, context window, tool support, pricing, cached
  pricing, currency, and remove action)
- Test connection loading/success/failure messages in the Providers screen
- provider catalog labels remaining unique after adding a second model
- every provider preset offering Fetch models
- max-token notices and blocked/incomplete outcomes never being treated as normal success
- MCP add/save/edit/remove flows, the validation error state, the confirmation
  cancel path, and the fact that opening the section never connects a server
- MCP editor accessibility names for every field and action
- MCP empty and configured-section snapshots

The accessibility check is enabled globally in the test harness. A failing
widget fails the test; there are no selectively disabled screens.

## Provider tests

No real credentials or external network access are used.

- `src/provider/probe.rs` runs connectivity and model-list probes against
  one-shot loopback HTTP servers: success, empty/malformed lists, `401`, `500`,
  connection refused, unsupported protocol, and URL/prefix construction.
- `src/provider/ollama_chat.rs` covers request body construction, message
  mapping, options, endpoint construction, NDJSON decoding across arbitrary
  chunk boundaries (multiple events, split events, trailing event, blank lines,
  CRLF, malformed and oversized payloads), final-event usage, incomplete
  streams, reasoning deltas, error events, and 404 model-not-found mapping.
- `src/provider/anthropic_messages.rs` covers native request construction,
  top-level system mapping, role/order preservation, empty-block handling,
  mandatory `max_tokens` default/validation, temperature range, explicit
  rejection of tool content, endpoint joining, required headers and absence of
  `Authorization` (`anthropic-version`, `x-api-key`), SSE text ordering across
  17-byte chunks, multiple events per chunk, thinking/signature/unknown events,
  malformed payloads, error events, incomplete streams, final usage, status
  mapping (400/401/403/404/429/5xx), cancellation of an open stream, and
  `ProviderController` dispatch.
- `src/provider/gemini_generate_content.rs` covers native body construction,
  system-instruction mapping, user/model roles, order and empty-block handling,
  consecutive same-role preservation, tool rejection, temperature/token bounds,
  model paths, `alt=sse`, `x-goog-api-key` with no `Authorization`, SSE ordering
  across split chunks and multiple parts, thought parts kept out of text,
  finish-reason classification, prompt blocking, usage mapping, malformed/error
  payloads, incomplete streams, status mapping, cancellation.
- `src/provider/openai_responses.rs` covers native `input`/`instructions`
  construction, ordering, empty and consecutive messages, tool rejection,
  settings bounds, custom endpoint prefixes, named SSE text/refusal/reasoning
  events, split chunks, no duplication from `.done` text, multiple parts,
  unknown and malformed events, failed/error events, token-limit and
  content-filter incompletes, refusal outcomes, unsupported tool output, usage,
  missing terminals, sanitized HTTP statuses, controller dispatch, and
  cancellation against loopback servers.
- `src/provider/probe.rs` also covers OpenAI Responses model URLs and parsing,
  Anthropic and Gemini model URLs, page
  parsing, cursor/page-token pagination, the page cap, generateContent-only
  filtering, empty/malformed lists, and sanitized auth/server errors.
- `src/provider/mod.rs` covers URL/auth validation, endpoint joining, auth
  headers, and rejection before network access.
- Provider UI tests verify catalog merging preserves existing entries and
  pricing, and that probe error messages contain no keys or URLs.

Live Ollama verification is opt-in and ignored by default:

```sh
NEXUS_OLLAMA_MODEL=llama3.2   cargo test --workspace live_ollama -- --ignored --nocapture
```

It requires an already-running local server and an already-installed model; it
never installs or downloads anything.

## Mission persistence tests

`src/mission.rs` tests the status transition tables, terminal-state closure, and input validation. `src/storage.rs` adds tests for:

- mission create/load/list and status transitions, including rejected illegal jumps
- ordered tasks, bounded task lists, task timestamps, deliverables, and dependencies
- cross-mission dependency rejection and duplicate dependency rejection
- appended task status history, including the initial pending event and read clamping
- attempt numbering, terminal-only completion, and double-finish rejection
- checkpoint and approval ownership, pending counts, and one-time decisions
- cascade deletion plus `clear_all_history`
- history-gate blocking of mission writes with reads still available
- input validation and list clamping
- upgrading a version-1 database to schema version 4 without recreating conversations
- adding privacy to a version-3 database with a `local_only` default
- mission and task definition updates, including rejected edits after planning
- task edits preserving status, assignment, and status history
- full-order task reordering and incomplete/duplicate order rejection
- deleting an unstarted task with dependency cleanup and ordinal compaction
- refusing task deletion after an attempt exists
- dependency edits rejecting self, duplicate, missing, cross-mission, and direct/indirect cycle edges
- failed dependency edits leaving the prior list and status history unchanged
- history-gate rejection for every definition mutation

These tests use an in-memory database except for the migration tests, which use isolated temporary directories. No mission execution, provider call, MCP connection, or network access is involved.

## Planning-domain tests

`src/mission/planning.rs` tests the Phase 5D.1 validation boundary without storage, UI, network, credentials, or a clock. The module currently contains 27 tests; the workspace suite contains 282 tests, with 281 passing and one ignored live-Ollama test as of this remediation. Coverage includes:

- draft/planned mission and pending-task requirements
- positive mission/task IDs, unique task IDs, contiguous ordinals, and mission ownership
- self, duplicate, missing, cyclic, exact-100, and over-100 dependency validation through `validate_planning_request`
- global, task-specific, duplicate, conflicting, missing-task, and pre-deduplication requirement bounds
- exact and overflow boundaries for tasks, agents, models, links, capabilities, evidence, locality, requirement groups, global requirements, preferences, identifiers, labels, versions, and currencies
- exact 1,000-task acceptance and 1,001-task rejection; exact 8/9 compatibility-evidence and 2/3 model-locality boundaries
- unique capabilities per agent with multiple evidence records retained
- sparse agent-model links; missing links stay unknown and provider data creates no links
- independent preservation of compatibility, capability, and locality claims, quality, source, timestamp, and version provenance
- absent, input-only, output-only, and both-present prices; missing, empty, and oversized currencies; no cost calculation
- oversized nested mission JSON rejected through the bounded serializer, with valid nested JSON preserved
- `None`, `Some(0)`, and positive budget limits kept distinct
- deterministic normalized output from equivalent shuffled agents, models, links, requirements, and preferences
- empty inventories remaining valid
- a source-level guard against clock, filesystem, network, process, environment, database, credential, Keychain, and HTTP access

No planning test evaluates compatibility, ranks candidates, selects an agent/model, creates a plan or approval, or persists data.

`src/ui/tests.rs` drives the headless Missions screen through AccessKit. It covers the empty mission and approval states, mission and task selection with keyboard activation, attempts/checkpoints/status history, the informational approval queue, deleted-mission recovery, bounded mission lists, and forced load errors that keep the last successful data. It also creates and edits a mission, changes privacy, rejects invalid JSON before saving, adds/edits/reorders/removes tasks, edits dependencies, rejects a dependency cycle before saving, and confirms mission deletion. Snapshot baselines are `missions-empty.png`, `missions-overview.png`, `mission-editor.png`, `task-editor.png`, `dependency-editor.png`, and `approval-queue.png`. The mission fixture pins display timestamps only in test builds so snapshots stay deterministic.

## MCP tests

`src/mcp/` tests cover configuration validation (STDIO command/arguments,
absolute working directory, environment-name syntax, HTTPS/loopback rules,
embedded credentials), disabled and duplicate connection rejection, spawn
failure sanitization, refused Streamable HTTP endpoints, status synchronization,
and a full STDIO lifecycle against a test-only fixture MCP server.

The fixture is a small Rust program compiled once by `rustc` into a temporary
directory. It answers `initialize`, `tools/list`, and `ping`, logs every method,
and is connected through the real `rmcp` client. The test asserts that the
negotiated protocol is `2026-07-28`, catalog metadata and capabilities arrive,
disconnect retains a stale catalog, and the fixture never receives `tools/call`,
`resources/read`, or `prompts/get`. No shell, Node, Python, external service, or
network access is involved.

A separate source-level test fails if the MCP module names tool execution,
resource, prompt, sampling, or elicitation methods. Streamable HTTP success
against a live server remains manually unverified; only URL validation and a
refused local connection are automated.

## Configuration recovery tests

`src/ui/recovery.rs` tests drive the real recovery window through `egui_kittest`
using isolated temporary configuration directories:

- the window appears for malformed TOML and renders no file contents
- Retry leaves the corrupt file byte-identical and recovers after the user fixes it
- Open configuration folder targets the configuration's parent directory
- Copy path emits the real configuration path as the copied text
- Reset requires confirmation and Cancel changes nothing
- Back up and reset produces exactly one backup with the original bytes before
  writing valid defaults and entering the workbench
- an unwritable directory prevents backup and no reset occurs
- an injected write failure leaves the original and the backup intact
- Quit exits without modifying the file

The macOS Finder launch and clipboard paste are exercised only for their path
and command values; the OS-level open/paste is not automated. No test touches the
real user configuration or login Keychain.

## Snapshots

Baselines live in `tests/snapshots/*.png`:

| Name | State |
| --- | --- |
| `chat-empty` | first run, callout visible, no conversations |
| `providers-default` | default local-gateway profile editor |
| `settings-history` | appearance/history settings, fixed data paths |
| `usage-empty` | no usage recorded |
| `recovery-config-error` | configuration recovery window |

Regenerate after an intentional visual change:

```sh
UPDATE_SNAPSHOTS=1 cargo test --workspace snapshot
```

On failure, inspect `tests/snapshots/<name>.diff.png` and `<name>.new.png`; do not raise tolerances to hide a real layout change.

Determinism rules for new snapshots:

- fixed window size (1080×720), dark theme, no system theme lookup
- fixed `/Users/example/...` data paths, never the real temp directory
- no timestamps, relative times, PIDs, or live telemetry values
- deterministic empty or seeded conversation fixtures

Rendering uses wgpu. A machine without a usable GPU adapter cannot render snapshots; run them where the release app runs. Cross-machine baselines can differ in font rasterization, so regenerate locally rather than copying PNGs between machines.

## Configuration recovery tests

`src/ui/recovery.rs` tests run against temporary directories only. They verify that retry and quit never modify the file, reset requires confirmation, a backup is byte-identical to the original, a failed backup aborts before writing, and error summaries never include configuration contents. They never touch the real configuration, the database, or the login Keychain.
