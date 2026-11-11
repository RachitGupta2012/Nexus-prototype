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

- Bounded PATH and known application discovery.
- Supported package-manager and process inventory where safe.
- Read only documented configuration metadata; redact secret-like values.
- Let the user add, ignore, and rescan integrations.
- Represent installed, running, authenticated, connectable, and usable as distinct states.

## Phase 3 — model intelligence

- Canonical model identities and provider-specific variants.
- Import provider model catalogs where documented.
- Add benchmark adapters only after verifying current APIs, terms, pricing, and rate limits.
- Store source, timestamp, confidence, and task category for every score.
- Combine external evidence with NEXUS's own observed outcomes.
- Never use one universal leaderboard for every task category.

## Phase 4 — agent integrations

- Implement a tested ACP client where supported.
- Implement MCP client/server capabilities where useful.
- Add documented native adapters for selected CLI/harness tools.
- Add direct model-provider integrations only where appropriate.
- Support timeouts, cancellation, output limits, and capability negotiation.
- Never claim automatic model switching if an external harness does not expose it.

## Phase 5 — mission engine and routing

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
