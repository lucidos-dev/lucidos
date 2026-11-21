# 0613: Engine boots without a model provider in every launch mode

- **Status**: Accepted
- **Date**: 2026-11-02
- **Supersedes**: ADR 0012's providerless dev/docker fail-fast distinction

## Context

A fresh source install could not reach Settings because engine construction
failed before a model provider was configured. Packaged launchers avoided this
only by setting an opt-in environment flag. Model availability must not decide
whether the workspace surface is available (see ADR 0135).

## Decision

All launch modes install `UnconfiguredProvider` when no provider is available.
Health succeeds and reports `llm_configured: false`. Chat emits a visible error
pointing to Settings → Models → Providers. Only explicit `LUCIDOS_MODEL=mock`
selects the test mock. `LUCIDOS_BOOT_WITHOUT_PROVIDER` is obsolete and ignored;
older launchers may continue passing it harmlessly.

Runtime provider changes rebuild the active provider: adding configuration
enables chat and removing the last provider restores the sentinel. Neither
transition requires a restart. Rebuild errors retain the current provider.

## Consequences

Source and packaged macOS/Linux installations can configure their first
provider inside Lucidos. Health measures engine availability independently of
model configuration. Providerless chats fail visibly instead of producing mock
content. The provider selection no longer has a fail-fast state or boot gate.
