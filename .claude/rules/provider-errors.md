---
paths:
  - "crates/lucidos-engine/src/llm/**"
---

# Provider errors

A failure from a model provider is a `llm::ProviderError` (ADR 0673).

- **Build it through the parser.** An HTTP error uses `ProviderError::from_http(provider, status, body)`. A mid-stream error frame uses `from_stream_error(provider, &error_json)`. Never `format!` a status, a code or "API error" into an error string.
- **Retry on the kind.** Loops call `error_is_retryable` or `should_retry_http`. Both read `kind.is_retryable()`, and an `Unknown` kind falls back to the provider's message. Do not add a provider wire code to `is_retryable_error`.
- **A new kind or code is a table row.** Add it to `ProviderErrorKind::from_code` and give the kind a `meaning` sentence. A new retryable meaning keeps a phrase `is_transient_error` matches.
- **Keep the typed errors that come first.** `SpendCapReached`, `ModelNotServed` and `ChatGptPlanError` are checked before a `ProviderError` is built.
- **Sign-in, discovery and telemetry endpoints are not model calls.** They stay out of this rule. A new one goes in `NOT_A_MODEL_CALL` in `provider_error_tests.rs`, which holds the default-deny guard.
