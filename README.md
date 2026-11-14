# NEXUS

NEXUS is a native Rust desktop workbench for model provider APIs. It connects to OpenAI-compatible Chat Completions endpoints, keeps conversations in local SQLite, stores provider keys in the macOS Keychain, and shows provider-reported token usage.

MCP connections, non-OpenAI protocol adapters, agent execution, and mission orchestration are **not** implemented. The app sends chat completions over the network only when you press Send.

## Requirements

- Stable Rust toolchain with Cargo on Apple Silicon macOS.
- macOS system SDK and linker tools (normally provided by Xcode Command Line Tools).

## Build and run

```sh
cargo run
```

Create an optimized executable:

```sh
cargo build --release
./target/release/nexus-app
```

The executable is `target/release/nexus-app`. No signed or notarized `.app` bundle is produced yet.

## Checks

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo build --release
```

## Screens

- **Chat** — conversation sidebar with search, inline rename, delete, clear-all, relative timestamps, streaming responses, system prompts, and per-message token/cost notes.
- **Providers** — add profiles from presets, edit endpoints and auth, maintain model catalogs and prices, test nothing automatically, and save or remove keys with the macOS Keychain.
- **Usage** — provider-reported token totals overall, per provider, per model, and per day. Costs are estimates from your configured prices and are labelled as such.
- **Resources** — NEXUS process RSS, CPU, uptime, and peak. Child processes are not included.
- **Settings** — appearance (System/Dark/Light), history and retention controls, tool-output saving, data paths with copy buttons, and history deletion.

## Data locations

On macOS:

- Configuration: `~/Library/Application Support/NEXUS/config.toml`
- Database: `~/Library/Application Support/NEXUS/nexus.db`
- Provider keys: macOS Keychain, service `com.nexus.app.credentials`, scoped per profile and endpoint origin.

No API key is written to the configuration file, the database, or logs.

## History and privacy

- With history enabled, completed conversations and provider usage are saved locally in SQLite (plaintext at rest, file permissions `0600`).
- With history disabled, chat works normally but conversations exist only in memory. No conversation, message, tool-call, or usage rows are written, and nothing returns after restart. Settings states that usage data is therefore unavailable in that mode.
- Search covers saved titles and message text only, capped at 200 results.
- **Clear all history** deletes conversations, messages, and conversation-linked usage. It does not touch providers, keys, or settings.

## Limitations

- Only the OpenAI-compatible Chat Completions protocol sends requests. Anthropic, OpenAI Responses, Gemini, and Ollama profiles can be configured but return an unsupported-protocol error if sent.
- MCP configuration types exist, but no MCP client, tool discovery, or tool execution is implemented.
- Usage cost is never invented: a value appears only when both provider usage and model prices exist.
- The repository roadmap still describes earlier agent-platform phases; see [docs/ROADMAP.md](docs/ROADMAP.md) for the Phase 4.5 supporting milestone and the preserved Phase 5/6 objectives.
