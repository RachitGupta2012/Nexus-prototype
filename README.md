# NEXUS

NEXUS is a native Rust desktop application for AI-agent orchestration. It includes bounded agent discovery, local evidence-based task recommendations, and an explicitly user-triggered OpenAI-compatible provider connection. Provider requests return text; they do not run agents or edit project files. Agent execution, mission orchestration, native non-compatible provider protocols, and child-agent monitoring are not implemented.

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

Set `RUST_LOG` to control structured log filtering (for example, `RUST_LOG=debug cargo run`). NEXUS does not scan automatically or start a background service. Agent discovery makes no network requests. Provider credentials are entered by the user and stored in the macOS Keychain; provider requests occur only after the user clicks Send task to provider. Standard Discovery checks eligible `PATH` entries and documented conventional roots only after a user request. Expanded System Discovery is a separate consent-gated option. The exact macOS locations and exclusions are listed in [the discovery catalog](docs/ADAPTERS.md#discovery-roots).

Tokio is deferred until a bounded asynchronous operation is introduced. Resource sampling uses a stoppable standard-library thread; explicit agent discovery uses a short-lived, cancelable standard-library thread.

## Task routing

The Routing screen accepts a bounded task description, a user-selected category, mandatory requirements, and optional preferences. Results use a small in-code capability catalog and the local registry snapshot. Current catalog evidence documents software-development workflows only; other capability dimensions remain unknown. Documentation-only claims yield tentative recommendations, while unknown mandatory requirements prevent a recommendation. Task text and manual selection remain in memory for the current UI session and are not logged or persisted. See [the architecture](docs/ARCHITECTURE.md#phase-3-task-routing) and [adapter evidence policy](docs/ADAPTERS.md#phase-3-capability-evidence-and-routing).

Agent rescans also check the exact supported macOS app bundles listed in [the discovery catalog](docs/ADAPTERS.md#supported-macos-application-bundles). App presence is recorded separately from CLI discovery and does not imply NEXUS can control the app.

## Provider connection

The Providers screen accepts one OpenAI-compatible Chat Completions endpoint and model identifier. This can be a compatible hosted API or a local gateway such as FreeLLMAPI; the protocol is not universal, so native Anthropic Messages, Gemini, and other distinct APIs are not claimed to work yet. Enter a provider-issued key yourself and save it to macOS Keychain. NEXUS neither searches for nor imports existing keys. On the Routing screen, **Send task to provider** is the explicit action that transmits the task text; remote use may incur charges. The response is displayed in NEXUS, but the provider does not edit files or execute an agent. See [provider and security boundaries](docs/SECURITY.md#explicit-provider-requests).

## Resource use

The Resource Monitor reports NEXUS RSS, not macOS physical footprint, and keeps ten minutes of samples in memory. Discovery is bounded and read-only; custom search folders authorize passive exact-name checks directly inside the selected folder only. Discovery never executes tools or inspects their credentials or configuration. Registry detections are not proof that an agent is authenticated, connectable, usable, or controllable by NEXUS. The targets in [docs/RESOURCE_BUDGET.md](docs/RESOURCE_BUDGET.md) remain targets, not guarantees.
