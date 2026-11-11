# Security and permissions

## Security defaults

- Discovery is read-only.
- No credential scraping, token extraction, or secret logging.
- Do not store secrets in project files, SQLite plaintext fields, logs, crash reports, or Git.
- Prefer macOS Keychain through a small credential-store abstraction.
- Do not ask for broad macOS permissions during bootstrap.
- Show the user what an integration will execute and which workspace it can access.
- Require explicit approval for sensitive actions, external side effects, or configured spending thresholds.

## Phase 2 discovery boundary

- Standard Discovery is the default, explicitly user-started scope. It checks exact names in at most 64 eligible PATH entries (16 KiB input), listed macOS conventional roots, and user-added custom folders. Ineligible, missing, skipped, and inaccessible roots are reported with a reason.
- Expanded System Discovery is a separate opt-in. Before it starts, NEXUS shows `/Applications`, `/opt`, and the current user's `~/Applications`, depth/entry/duration caps, exclusions, and privacy boundary. It requires affirmative consent. Declining leaves Standard available and causes no automatic repeat prompt. A new approval version is required after material scope changes.
- Expanded traversal is capped at depth 4, 128 directories, 3,000 entries, and 10 seconds. It skips system/protected and personal-data directory names, known remote-volume paths, and all directory symlinks. It exposes progress and can be canceled. It never scans `/`, other user home folders, Documents, Downloads, Desktop, Pictures, Photos, Movies, Music, Library, or system-protected trees.
- Custom folder selection uses a native macOS folder dialog. NEXUS accepts only existing absolute directories with no symlink in their path, and rejects filesystem types not known to be local. A custom path permits only passive, nonrecursive exact-name checks directly within that path. Removing it excludes it from future scans.
- No mode executes candidates, version commands, package managers, shells, or agents; inspects process lists, configurations, credentials, Keychain, sessions, or file contents; or makes network calls.
- Symlink targets must remain within their explicitly authorized containment root; otherwise the candidate is reported as skipped. Discovery never follows directory symlinks during expanded traversal.
- Version verification is separate from discovery. NEXUS does not launch version commands; any manually recorded version is labeled user-reported.
- The registry stores NEXUS-owned metadata only. Its destination and parent components are checked for symlinks; writes use a same-directory temporary file and atomic rename. Registry errors are displayed without stopping the app.
- “Detected” only means a candidate executable path was observed. Running, authenticated, connectable, and usable remain unchecked.

## Permission tiers

1. **Inventory** — list known applications, executables, processes, and supported metadata.
2. **Execution** — launch an approved integration in an approved workspace.
3. **Advanced OS access** — Accessibility, Automation, Screen Recording, or protected-file access, only for a feature that genuinely requires it and after explicit consent.

## Process execution

- Use argument arrays rather than shell string concatenation.
- Set working directories explicitly.
- Apply timeouts, cancellation, output-size limits, and environment-variable allowlists.
- Do not inherit secrets into child processes unnecessarily.
- Treat agent output and discovered configuration as untrusted data, not instructions to NEXUS.

## Network and provider usage

- Respect provider terms, quotas, and rate limits.
- Show estimated cost only when pricing and usage data are available; label estimates.
- Never bypass authentication, usage limits, or access controls.
