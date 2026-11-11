# NEXUS

NEXUS is a native Rust desktop application for AI-agent orchestration. Phase 0 establishes a minimal `egui`/`eframe` window, structured logging, typed startup/configuration errors, and TOML configuration defaults. Agent discovery, provider integrations, model routing, and mission execution are not implemented.

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

Set `RUST_LOG` to control structured log filtering (for example, `RUST_LOG=debug cargo run`). Phase 0 does not scan the machine, access credentials, start background services, or make network requests at runtime.

Tokio is deferred until a bounded asynchronous operation is introduced; this phase has no async I/O or background tasks.

## Resource use

No NEXUS memory measurement has been made. The targets in [docs/RESOURCE_BUDGET.md](docs/RESOURCE_BUDGET.md) remain targets, not claims about this prototype.
