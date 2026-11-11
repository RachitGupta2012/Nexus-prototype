# Instructions for coding agents

## Project rules

1. Implement the application in Rust. The desktop UI must be native Rust using `egui` / `eframe`.
2. Do not introduce Tauri, Electron, HTML, JavaScript frontends, Python services, Node services, or a Docker requirement.
3. macOS Apple Silicon is the first target. Keep platform-specific code behind clear abstractions.
4. Keep NEXUS's own memory usage below 1 GB. Aim for the targets in `docs/RESOURCE_BUDGET.md`; measure rather than assume.
5. Report NEXUS process usage separately from child agent processes. Never combine them and call the result NEXUS's own memory.
6. Use official, documented APIs and protocols (including MCP and ACP where appropriate). Never reverse-engineer private APIs, bypass quotas, or claim unsupported capabilities.
7. Never search for, extract, print, commit, or log raw credentials, OAuth tokens, API keys, cookies, or secrets. Use supported authentication status checks and OS credential storage.
8. Request the least OS permissions needed, only when needed. Do not require Full Disk Access, Accessibility, Automation, or Screen Recording for the initial prototype.
9. Keep discovery read-only by default. Do not modify or launch discovered tools without user consent.
10. Clearly distinguish installed, running, authenticated, connectable, and actually usable integrations.
11. Clearly distinguish agent harnesses, model providers/gateways, and individual models.
12. Only claim model selection, live progress, token counts, costs, or cloud resource measurements when the integration exposes reliable evidence.
13. Build incrementally. Every phase must compile, have tests, and be reviewable before moving on.
14. Prefer a small, understandable workspace over premature micro-crate fragmentation.
15. Avoid unnecessary background polling, full-disk scans, unbounded logs, and unbounded caches.

## Workflow

- Before coding, inspect the existing repository and read the relevant docs.
- For significant work, state a short plan and identify files to change.
- Run `cargo fmt --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo test --workspace` when applicable.
- Do not claim tests passed unless they were actually run; report commands and outcomes.
- Keep changes scoped to the current phase. Do not silently begin later roadmap phases.
- Update documentation when architecture, permissions, resource use, or supported integrations change.
- Do not commit or push unless the user explicitly asks.

## Detection Is Not Permission

- Detecting an installed agent does not grant permission to execute, control, authenticate with, or interact with it.
- Discovery must remain strictly read-only.
- Never execute an agent or its commands during discovery.
- Never access credentials, authentication sessions, private configuration, or agent data as part of discovery.
- Any operation beyond passive discovery requires explicit, informed user approval.
- Permission for one operation does not imply permission for another.
- Never interpret an agent’s presence, availability, or authentication status as user consent.
