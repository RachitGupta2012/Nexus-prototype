# Codex first task — Phase 0 only

Read `AGENTS.md` and all documents in `docs/` first.

## Objective

Create the smallest working native Rust foundation for NEXUS. Do not implement discovery, benchmarks, agent integrations, model routing, or mission orchestration yet.

## Required work

1. Inspect the repository, operating system, Rust toolchain, and available targets.
2. Verify current maintained versions and compatibility of candidate crates from official sources. Do not blindly pin stale versions from this brief.
3. Propose a concise implementation plan and workspace structure.
4. Create the Cargo workspace and minimal native egui/eframe application.
5. Add structured logging, typed errors, and a simple configuration boundary.
6. Add a basic window with NEXUS title, app version, and placeholder navigation or status panel.
7. Add unit tests for any core types/configuration introduced.
8. Add CI for formatting, clippy, and tests.
9. Document exact commands to build, run, test, and package locally.

## Constraints

- Rust only for application code.
- Native egui/eframe GUI; no Tauri, Electron, web frontend, Python, Node, or Docker.
- Keep dependencies minimal and justify each dependency.
- Do not add a background daemon or localhost server.
- Do not access credentials or scan the user's machine in this phase.
- Do not claim a memory target has been met without measuring it.
- Do not commit or push.

## Verification

Run and report the actual results of:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo build --release
```

If a command cannot run in the environment, explain why and provide the exact local verification step.

## Completion report

Summarize:
- Files created/changed.
- Workspace structure and key dependency choices.
- Build/test commands and actual results.
- Any platform limitations.
- Next recommended task (Phase 1 only after review).
