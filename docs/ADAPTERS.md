# Integration and adapter policy

## Supported integration families

- ACP: agent-client protocol integrations where an official, maintained implementation is available.
- MCP: tool and context integrations using documented protocol behavior.
- Direct provider APIs: model catalogs and inference only where official APIs support them.
- Native CLI adapters: documented command-line interfaces with stable, testable behavior.

## Adapter contract

Each adapter should declare:

- Integration identity and version.
- Whether it is installed, running, authenticated, connectable, and usable.
- Supported operations and capability limitations.
- Whether model selection is supported.
- Authentication method without exposing credential material.
- Timeout, cancellation, and output-limit behavior.
- Evidence source and last verification time.

## Phase 2 discovery candidates

The bounded catalog currently contains these agent harnesses/agent CLIs and exact executable basenames:

| Candidate | Executable | Classification and evidence |
|---|---|---|
| Codex CLI | `codex` | Agent harness; vendor documentation is recorded as documentation evidence. |
| Claude Code | `claude` | Agent harness; vendor CLI documentation is recorded as documentation evidence. |
| OpenCode | `opencode` | Agent harness; CLI docs describe interactive and print modes and ACP serving. |
| Pi coding agent | `pi` | Agent harness candidate; `pi` is a generic basename, so a match is always marked ambiguous. |
| Gemini CLI | `gemini` | Google Gemini CLI, documented as an open-source terminal agent. It is only a detected executable, not a provider connection. |
| GitHub Copilot CLI | `copilot` | GitHub's terminal-native coding assistant with agentic capabilities. |
| Aider | `aider` | Coding-assistant harness documented to accept source files and make edits. |
| Amp CLI | `amp` | Amp's coding-agent CLI; `amp` is a generic basename, so matches are always marked ambiguous. |
| Goose CLI | `goose` | Goose's documented general-purpose AI agent CLI. |

The catalog does not include model providers, API gateways, model aggregators, or ordinary developer tools. For example, OpenRouter is a provider/gateway and belongs in a later provider registry. Candidate identity references are linked from each record's Documentation field. The catalog interfaces are documentation claims, not verified capabilities of the installed version. No candidate is claimed running, authenticated, connectable, usable, or controllable.

## Discovery roots

Standard Discovery is the default and is explicitly started by the user. It checks exact candidate names directly in eligible PATH entries and these conventional macOS roots. A root is checked once even if it appears in PATH and in the catalog.

| macOS root | Why included |
|---|---|
| `/opt/homebrew/bin` (containment: `/opt/homebrew`) | Conventional Apple Silicon Homebrew executable directory. |
| `/usr/local/bin` (containment: `/usr/local`) | Conventional Intel Homebrew and local CLI directory; may also be used on Apple Silicon. |
| `/usr/bin` (containment: `/usr`) | macOS system PATH executable directory. |
| `~/bin` | Common user-level CLI location. |
| `~/.local/bin` | XDG-style user CLI location. |
| `~/.cargo/bin` | Cargo-installed executables. |
| `~/.npm/bin`, `~/.npm-global/bin` | Common npm user-global binary directories; NEXUS does not query npm for a configured prefix. |
| `~/.bun/bin` | Bun-installed executables. |
| `~/.volta/bin` | Volta shims. |
| `~/.asdf/shims`, `~/.local/share/mise/shims` | asdf and mise command shims. |
| `~/.yarn/bin` | Yarn user-level executable directory. |
| `~/.local/share/pnpm`, `~/Library/pnpm` | Common pnpm global executable directories. |
| `~/.linuxbrew/bin` | Home-directory Homebrew layout. |
| PATH entries inside `/opt/homebrew`, `/usr/local`, `/usr`, or eligible user-managed roots including `~/.nvm`, `~/.fnm`, and the listed shims | Captures version-manager layouts such as `~/.nvm/versions/.../bin` without invoking managers or traversing those trees. |

PATH input is capped at 16 KiB and 64 entries. Relative PATH entries and directories outside those allowlisted roots are skipped with a reason. Missing roots are recorded as missing, unsafe/symlinked roots as skipped, and permission failures as inaccessible. `~/Documents`, `~/Downloads`, `~/Desktop`, `~/Pictures`, `~/Library` generally, `/System`, `/Network`, `/Volumes`, and `/net` are not standard search roots. npm/Homebrew/package-manager commands are never executed to discover prefixes. PATH values can name an otherwise unconventional directory; that directory is checked only when it is within the documented eligible roots.

This list is intentionally non-exhaustive. Custom installations outside these roots require an optional custom search location or may not be detected.

## Expanded System Discovery

Expanded Discovery is an explicit setting with a consent dialog that names its roots before scan. It adds bounded recursive name/metadata checks under `/Applications`, `/opt`, and `~/Applications`. It excludes system-protected areas, Library and configuration/data subtrees, credentials-related folder names, personal documents/media/download folders, remote-volume paths, and directory symlinks. It is limited to depth 4, 128 directories, 3,000 entries, and 10 seconds; it runs on a cancelable worker and reports progress. Scope approval is stored with a scope version; a material scope change requires new approval. Declining leaves Standard Discovery available and does not trigger automatic repeat prompts.

Expanded Discovery is not a full-disk scan and does not guarantee discovery. It may miss unusual installs. It does not inspect file contents, agent config, authentication, Keychain, process lists, or credentials. It makes no network requests and never executes any tool.

## Custom search locations

On macOS, the native folder picker lets the user add up to 16 custom directories. NEXUS validates an existing absolute real directory, rejects symbolic-link path components and known/identified remote or unrecognized filesystems, then persists the path in its own registry. A custom location authorizes only passive exact-name checks directly inside that directory; there is no recursion. Remove excludes the path from later scans. This does not grant execution or control permission.

Do not assume an integration is available or supports a feature until verified against its current documented interface. Version checks are a separate user-run operation, not part of discovery.

OmniRush and any other proprietary or less-documented tool must remain unintegrated until a supported interface is confirmed.

## Model selection truthfulness

If an external harness exposes model selection, NEXUS may use it. If it does not, NEXUS may recommend a model but must not claim to have switched it. Never reverse-engineer private APIs or bypass service restrictions.

## Phase 3 capability evidence and routing

`capability.rs` provides typed capability identifiers and a small curated catalog separate from discovery records. The initial claims are vendor-documented software-development workflows for the nine existing harness candidates. They do not claim individual file operations, research, data analysis, document processing, writing, structured output, or tool-use support. Capability is never inferred from executable presence.

Evidence states are Verified, Documented, User-reported, Inferred, Unsupported, and Unknown. The source, evidence state/method, optional version, review date, concise claim, and limitation are retained. The current catalog contains documentation claims only. Routing may treat recent documented claims as tentative, but only exact version-matched Verified claims can produce Supported. A missing version is explicitly version-unverified; evidence older than 365 days, undated, future-dated, for another version, or in conflict cannot establish eligibility. The date cutoff is a simple deterministic freshness rule, not a claim that vendor behavior changes on a schedule.

Routing accepts a typed task, immutable discovered-record snapshot, catalog, and explicit timestamp. It ranks deterministically without arbitrary scores, preserves semantic ties, and reports unknown requirements rather than treating them as passing. Manual selection is session-only and changes no evidence or registry state. This feature does not execute agents or authorize execution.

## Supported macOS application bundles

Each explicit agent rescan also checks the exact bundle paths `DeepAI Code.app` and `OpenCode.app` directly under `/Applications` and `~/Applications`. This check is nonrecursive, bounded to two roots and two names, and reads filesystem metadata only. It does not inspect `Info.plist`, application contents, configuration, credentials, or running processes. The current list is curated and non-exhaustive. A found application is displayed separately from a detected CLI and is not claimed controllable or usable by NEXUS.

## Explicit API provider connector

The initial provider client supports OpenAI-compatible Chat Completions endpoints, including compatible gateways such as FreeLLMAPI. Configure a base URL and model identifier (a gateway may document a selector such as `auto`). NEXUS sends `{model, messages, stream:false}` and reads the first text choice. This protocol is not universal: native Anthropic Messages, Gemini `generateContent`, and other distinct provider APIs are not supported by this connector. Provider adapters should be added only against official documented APIs, with protocol-specific authentication, limits, error handling, and tests; do not claim universal market coverage.

The user enters a provider-issued API key and explicitly saves or removes it. On macOS it is stored in Keychain under the configured endpoint origin, so changing hosts does not reuse the old host's key. The value is read only when the user explicitly submits a task. Remote endpoints require HTTPS; loopback HTTP is allowed for local gateways. The request does not follow redirects, has a 120-second total timeout, and reads at most 2 MiB of response data. Task content is not logged or persisted by this connector. No key is searched for or imported.

This is a model response connection, not an agent harness. It does not read a workspace, write files, run commands, or execute a discovered CLI. It does not claim the provider selected a particular upstream model beyond the configured identifier. Agent execution and workspace operations require separately designed adapters and explicit authorization.
