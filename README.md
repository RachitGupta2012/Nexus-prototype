# NEXUS

NEXUS is a native Rust desktop workbench for model provider APIs. It connects to OpenAI-compatible Chat Completions endpoints, native Ollama, and the native Anthropic Messages API, keeps conversations in local SQLite, stores provider keys in the macOS Keychain, and shows provider-reported token usage.

MCP server configuration, explicit read-only connections, protocol negotiation, capability display, and tool-metadata discovery are implemented. MCP tool execution, resource reads, prompt retrieval, agent execution, and mission orchestration are **not** implemented. The app sends chat completions over the network only when you press Send.

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

UI interaction and PNG snapshot tests run as part of `cargo test`; see [docs/TESTING.md](docs/TESTING.md) for snapshot regeneration.

## Configuration recovery

A configuration file that cannot be read or parsed no longer stops NEXUS with a terminal error. The application opens a recovery window with the file path, a sanitized error (line and column only, never file contents), and four actions: retry without modifying the file, open the containing folder, back up the original file and reset to defaults after explicit confirmation, or quit. Resetting never deletes Keychain credentials, the database, or saved conversations.

## Screens

- **Chat** — conversation sidebar with search, inline rename, delete, clear-all, relative timestamps, streaming responses, system prompts, and per-message token/cost notes.
- **MCP** — add, edit, and remove STDIO or Streamable HTTP servers; connect or disconnect explicitly; inspect negotiated protocol, server identity, capabilities, and discovered tool metadata; refresh the catalog; and see clearly that tools are discovered but not executable.
- **Providers** — add profiles from presets, edit endpoints and auth, maintain model catalogs and prices, run a no-generation **Test connection** probe, **Fetch models** for OpenAI-compatible, Ollama, Anthropic, and Gemini endpoints, review discovered models before adding them, and save or remove keys with the macOS Keychain.
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

- OpenAI-compatible Chat Completions, native OpenAI Responses (`/v1/responses`), native Ollama (`/api/chat`), native Anthropic Messages (`/v1/messages`), and native Gemini Generate Content (`:streamGenerateContent?alt=sse`) send requests.
- Connectivity and model discovery are available for OpenAI-compatible, Ollama, and Anthropic endpoints; a successful probe proves the endpoint answered a model-list request, not that inference works.
- Anthropic, Gemini, and OpenAI Responses tool use are not implemented; chat requests carrying tools or tool results are rejected instead of being silently reshaped. Responses output containing tool calls fails with a clear unsupported-operation error.
- Gemini safety blocks, recitation stops, OpenAI refusals/content filters, and token-limit truncation are shown as such rather than as ordinary completed answers.
- Model discovery never installs models, and NEXUS never starts or configures Ollama. Start Ollama and install models yourself.
- Mission persistence exists as a durable schema and bounded store only. There is no mission composer, mission engine, agent loop, approval execution, or Mission Control UI yet.
- MCP support is read-only: the official `rmcp` client negotiates the protocol (including the stateless 2026 revision), lists tool metadata, and never calls a tool, reads a resource, retrieves a prompt, samples, or elicits. Nothing connects automatically and no reconnect occurs.
- MCP servers run only when you press **Connect**. STDIO children receive a minimal environment (`NEXUS_MCP_CLIENT` plus any names you allowlist), never a shell; Streamable HTTP rejects redirects and plain HTTP outside loopback. No MCP credential is stored.
- Discovery is bounded to 1,000 tools, 10 pages, 256-byte names, 8 KiB descriptions, 256 KiB per schema, with 15-second connect and 20-second discovery timeouts.
- Usage cost is never invented: a value appears only when both provider usage and model prices exist.
- The repository roadmap still describes earlier agent-platform phases; see [docs/ROADMAP.md](docs/ROADMAP.md) for the Phase 4.5 supporting milestone and the preserved Phase 5/6 objectives.
