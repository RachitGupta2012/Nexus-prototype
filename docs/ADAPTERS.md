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
