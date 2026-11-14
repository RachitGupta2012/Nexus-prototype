# Roadmap

## Phase 0 — repository and technical foundation

- Inspect repository and toolchain.
- Verify current official Rust SDKs/crates and platform support before choosing versions.
- Establish the Cargo workspace, formatting, linting, tests, and CI.
- Build a minimal native egui/eframe application that launches on Apple Silicon macOS.
- Add structured logging and clean error handling.
- Document build/run/test commands.

**Exit criteria:** clean build, app launches, CI checks are configured, and tests pass.

## Phase 1 — resource telemetry

- Measure NEXUS process memory and CPU.
- Enumerate child processes launched by NEXUS and report them separately.
- Add a small Mission Control resource panel.
- Validate measurements against macOS Activity Monitor.
- Add bounded refresh intervals and memory regression checks.

## Phase 2 — discovery and registry

- Maintain a documented bounded catalog of verified agent harness/CLI identities, including Codex CLI, Claude Code, OpenCode, Pi, Gemini CLI, GitHub Copilot CLI, Aider, Amp, and Goose.
- Standard Discovery checks exact executable names in eligible PATH roots plus documented Homebrew/npm/Cargo/Bun/Volta/pnpm and other common user-level binary locations only on explicit request.
- Offer a separate, explicitly approved Expanded scope over named local software-install roots, with traversal/time/entry limits, exclusions, progress, and cancellation.
- Allow optional user-authorized custom local folders for direct exact-name checks only; reject unsafe paths and remote/unrecognized filesystems.
- Provide location evidence and skip reasons in a scrollable registry UI, and keep sidebar navigation fixed.
- Keep a NEXUS-owned registry with evidence, last-scan metadata, custom locations, and scope approval version.
- Let the user ignore/restore candidates and rescan.
- Keep version verification separate and user-run; never execute candidate or version commands during discovery.
- Do not invoke package managers, shells, or candidates, or inspect process lists, agent configuration, credentials, Keychain, authentication sessions, file contents, or unrelated personal data.
- Detection is not permission: no presence result enables execution, authentication, or control.
- Represent installed, running, authenticated, connectable, and usable as distinct states.

**Exit criteria:** registry claims distinguish observations, vendor documentation, user-reported information, and unknown values; detected candidates are not represented as integrated or usable; no agent is executed or contacted.

## Phase 3 — intelligent agent routing

- Represent bounded, session-only tasks with user-confirmed categories, mandatory requirements, and optional preferences.
- Maintain a typed, curated capability catalog with provenance and freshness evidence, separate from executable discovery.
- Recommend only from detected, unambiguous, non-ignored candidates using a deterministic local comparison.
- Explain supported, tentative, insufficient-evidence, rejected, and excluded outcomes; preserve ties and allow a session-only manual override.
- Keep execution authorization false; do not execute agents or persist task descriptions.

**Exit criteria:** recommendations are reproducible and explainable; unknown mandatory requirements never pass; no agent execution, authentication, network access, or registry schema change is introduced.

## Phase 4 — agent integrations

- Implement a tested ACP client where supported.
- Implement MCP client/server capabilities where useful.
- Add documented native adapters for selected CLI/harness tools.
- Add direct model-provider integrations only where appropriate.
- Support timeouts, cancellation, output limits, and capability negotiation.
- Never claim automatic model switching if an external harness does not expose it.

### MCP read-only client foundation (implemented)

- Users add, edit, enable/disable, and remove STDIO or Streamable HTTP server entries.
- Connection is explicit; protocol negotiation, identity, capabilities, and bounded tool metadata are displayed through the official `rmcp` client.
- Tools are marked discovered but not executable. There is no `tools/call`, resource read, prompt retrieval, auto-connect, or reconnect.
- Remaining MCP work: tool-call approval loop, resource/prompt browsing, and MCP server mode. These stay in Phase 5/6 and are not part of this foundation.

### Provider connector foundation (implemented incrementally)

- The current app includes a user-configured OpenAI-compatible Chat Completions connection and macOS Keychain storage for a manually supplied API key.
- This supports compatible endpoints/gateways such as FreeLLMAPI; native protocols for other vendors remain future work and must use official documentation.
- Provider responses are text only. This foundation does not execute an agent, inspect a workspace, or edit files.

## Phase 4.5 — UI, settings, history, and usage polish (supporting milestone)

- Add a dedicated Settings screen: appearance (System/Dark/Light), history retention, tool-output saving, data locations, and Keychain guidance.
- Enforce the history switch at the storage boundary. With history off, chat continues in memory and no conversation, message, tool-call, or usage rows are written.
- Add bounded conversation search over titles and message text (200 results), inline rename, delete, clear-all, and relative timestamps.
- Add a usage dashboard with totals, per-provider, per-model, and per-day views. Costs remain labelled estimates derived from user-configured prices.
- Add first-run guidance and consistent empty states.

**Exit criteria:** history-off leaves no conversation-linked rows and nothing returns after restart; search is bounded and deterministic; usage views stay bounded; formatting, Clippy with `-D warnings`, tests, and the release build pass.

## Phase 5 — mission engine

### Mission persistence foundation (implemented)

- SQLite schema v2 for missions, ordered tasks, task attempts, checkpoints, and approval records.
- Closed status-transition tables, cascade deletion, history-write gating, and bounded reads/writes.
- No mission UI, decomposition, scheduling, provider/tool execution, or approval enforcement. These remain next.

- Task schema: objective, constraints, workspace, budget, privacy, deliverables.
- Manual Master Agent selection and automatic selection.
- Filter incompatible candidates before scoring.
- Score agent harness and model separately.
- Use one agent when sufficient; parallelize only when it adds value.
- Add approval gates for costly or sensitive actions.
- Add retries, escalation, cancellation, checkpoints, and review.

## Phase 6 — Mission Control

- Mission composer and task list.
- Agent/model inventory.
- Mission timeline and verified events.
- Approval queue.
- Resource dashboard and integration settings.
- Explain why a candidate was selected and show evidence provenance.

## End-to-end acceptance scenario

A small coding task is submitted. NEXUS discovers supported installed integrations, shows available models and evidence, recommends a team with an explanation, requests approval when policy requires it, delegates through a supported protocol, displays verified progress and local resource metrics, runs a review, and produces a final report.

Test alongside several independent agent processes. Verify NEXUS's own memory remains below the 900 MB critical threshold. Do not claim this until measured.
