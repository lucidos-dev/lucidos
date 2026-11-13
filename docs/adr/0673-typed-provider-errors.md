# 0673: Every model provider failure is a ProviderError with a known-or-unknown kind: the retry loops branch on it and the user reads its meaning

- **Status**: Accepted
- **Date**: 2026-11-13

Plan: `docs/plans/2026-11-13-typed-provider-errors.md`.

## Context

A turn failed on its first attempt with `{"type":"error","error":{"type":"api_error","message":"upstream error"}}`.
The Anthropic stream reader formatted that frame as text. The retry check then
searched the text for known words, found none, and gave up with no retry. The
user saw the raw frame and no hint of what it meant.

Each provider names failures its own way (`api_error`, `server_error`,
`INTERNAL`, a bare HTTP status), and each call site formatted its own message.
Retry decisions read those messages back as text, so every new wire code needed
a new word in a list.

## Decision

Every failure from a model provider is a `llm::ProviderError`: a provider label,
a `ProviderErrorKind`, the raw code and message, the HTTP status and the attempt
count. `Unknown(raw)` keeps a code the table does not name. One tolerant parser
reads the Anthropic, OpenAI and Gemini envelopes. The retry loops downcast to
the type and branch on `kind.is_retryable()`.

Its `Display` shows the provider's own words first, then one sentence on what
they mean, then the attempt count. The wire stays a string, so
`ResponseFailed.error` carries the rendered text and no frontend changes.

## Rationale

- **The kind is the contract.** A retry loop that branches on text breaks
  whenever a provider words an error differently. A retry loop that branches on
  an enum cannot miss a row, because the meaning table is an exhaustive `match`.
- **Unknown is explicit.** A code outside the table lands in `Unknown` with its
  raw text. The user loses only the gloss, never the information. Its retry
  falls back to the provider's message, since a local server or proxy often
  sends a bare "rate limit exceeded" with no code.
- **The label names the backend that failed.** OpenRouter, xAI, local and
  ChatGPT-plan calls share the OpenAI wire, so they pass
  `ProviderKind::label`, never a fixed "OpenAI".
- **One parser, not one per provider.** The three envelopes differ only in which
  field names the failure (`type`, `code`, `status`). A single table covers all
  of them, so a fourth provider adds table rows and no new parser.
- **It meets the rule for a custom error type.** `rust.md` allows one when a
  caller branches on it. The retry loops do.

## Consequences

- Retryable meaning sentences keep phrases `is_transient_error` matches
  ("temporarily unavailable", "overloaded", "rate limit", "timed out"), so
  notification dedup still reads a rendered failure as transient. A test pins it.
- `is_retryable_error(text)` stays for errors that are not provider errors
  (network, stream read). It no longer lists provider wire codes.
- `should_retry_http` also retries when the body names a retryable kind.
- A source guard test fails when a model call formats a status into its own
  error. `.claude/rules/provider-errors.md` states the rule for new providers.
- Spend caps, unserved models, prefill refusals and the ChatGPT plan's own codes
  keep their types and their precedence.

## Alternatives considered

- **Add `api_error` to the retry word list.** Fixes the seed bug only. The next
  provider wording needs the same patch, and the user still sees no meaning.
- **One kind enum and one parser per provider.** The plan said this. The three
  wire shapes overlap enough that separate parsers duplicated the table.
- **A structured `ResponseFailed` field for the kind.** Possible later. The
  rendered text already carries the meaning, so it is not needed now.
- **A typed enum for every error in the engine.** No rule anyone can keep. Tool,
  git and API errors have no caller that branches on a kind.
