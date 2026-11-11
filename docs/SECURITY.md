# Security and permissions

## Security defaults

- Discovery is read-only.
- No credential scraping, token extraction, or secret logging.
- Do not store secrets in project files, SQLite plaintext fields, logs, crash reports, or Git.
- Prefer macOS Keychain through a small credential-store abstraction.
- Do not ask for broad macOS permissions during bootstrap.
- Show the user what an integration will execute and which workspace it can access.
- Require explicit approval for sensitive actions, external side effects, or configured spending thresholds.

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
