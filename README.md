# NEXUS

NEXUS is a native Rust desktop application for AI-agent orchestration. Phase 0 established the `egui`/`eframe` foundation; Phase 1 added an in-process Resource Monitor for NEXUS's own RSS, CPU, PID, uptime, and bounded recent history. Phase 2 adds explicit, read-only discovery of selected agent CLI candidates and a local evidence-based registry. Agent execution, provider integrations, model routing, mission execution, and child-agent monitoring are not implemented.

## Requirements

- Stable Rust toolchain with Cargo and the `aarch64-apple-darwin` target on Apple Silicon macOS.
- macOS system SDK and linker tools (normally provided by Xcode Command Line Tools).

The CI workflow runs on macOS and checks formatting, Clippy, and unit tests.

## Build and run

From the repository root:

```sh
cargo run
```

Create an optimized executable:

```sh
cargo build --release
```

The executable is `target/release/nexus-app`. This phase does not create a signed or notarized `.app` bundle; packaging requires a later distribution decision.

Build without telemetry for a repeatable enabled/disabled resource comparison:

```sh
cargo build --release --no-default-features
```

## Checks

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo build --release
```

## Configuration and logs

Configuration is loaded from `~/Library/Application Support/NEXUS/config.toml` on macOS. If that file is absent, built-in defaults are used. A minimal override is:

```toml
welcome_message = "Your local agent workspace is ready to configure."
```

Set `RUST_LOG` to control structured log filtering (for example, `RUST_LOG=debug cargo run`). NEXUS does not scan automatically, access credentials, start background services, or make network requests. Standard Discovery checks eligible `PATH` entries and documented conventional roots only after a user request. Expanded System Discovery is a separate consent-gated option. The exact macOS locations and exclusions are listed in [the discovery catalog](docs/ADAPTERS.md#discovery-roots).

Tokio is deferred until a bounded asynchronous operation is introduced. Resource sampling uses a stoppable standard-library thread; explicit agent discovery uses a short-lived, cancelable standard-library thread.

## Resource use

The Resource Monitor reports NEXUS RSS, not macOS physical footprint, and keeps ten minutes of samples in memory. Discovery is bounded and read-only; custom search folders authorize passive exact-name checks directly inside the selected folder only. Discovery never executes tools or inspects their credentials or configuration. Registry detections are not proof that an agent is authenticated, connectable, usable, or controllable by NEXUS. The targets in [docs/RESOURCE_BUDGET.md](docs/RESOURCE_BUDGET.md) remain targets, not guarantees.
