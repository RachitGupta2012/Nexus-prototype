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

## Initial investigation candidates

Investigate OpenCode, Free Pi CLI, OpenRouter, and other user-installed systems. Do not assume an integration is available or supports a feature until verified against its current documented interface.

OmniRush and any other proprietary or less-documented tool must remain unintegrated until a supported interface is confirmed.

## Model selection truthfulness

If an external harness exposes model selection, NEXUS may use it. If it does not, NEXUS may recommend a model but must not claim to have switched it. Never reverse-engineer private APIs or bypass service restrictions.
