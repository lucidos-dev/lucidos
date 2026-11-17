# 0717: The compiler classifies every diagnostic log value as shareable or private

- **Status**: Accepted
- **Date**: 2026-11-16

## Context

Diagnostic bundles can leave the machine (ADR 0716). Secret masking cannot
recognize conversation text in a log message. A promise to remove that text
needs a structural rule at every log site.

## Decision

A compile-checked `log!` proc macro requires every interpolated value to
implement a classified log value trait. Shareable types render normally;
all other values require `private(x)` and reach the maintainer as `<text N chars>`.

## Rationale

The compiler can reject missing classifications before a log site ships.
Code templates describe operations without exposing the text those operations
process. Types classify values consistently across destinations and processes.

## Consequences

- A small `lucidos-diagnostic-log` crate owns the sanitizer, writer, typed
  records, renderers and panic hook. A proc-macro sibling checks templates
  and inline captures. Gateway and client never need the engine dependency.
- Shareable types include ids, numbers, booleans, durations, timestamps, enum
  and kind names, and HTTP status. Errors contribute typed kinds such as
  `io::ErrorKind`, SQL state or an error variant.
- Bare `String`, `&str`, paths and error display messages fail compilation.
  `private(x)` classifies them explicitly.
- Each stored line keeps its code template and classified values alongside
  rendered text. Secret masking applies before storage and again before reading.
- Maintainer rendering replaces private values with `<text N chars>`.
  Owner-workspace rendering retains sanitized private values.
- Client logs classify category and codes as shareable and messages as private.
- Panic hooks classify code locations and thread names as shareable, and panic
  messages as private. Stderr tees store stray output as untyped lines.
- Deny `clippy::print_stdout` and `clippy::print_stderr` in the engine and
  gateway, with allowances confined to logging internals.
- Rename existing `log!` sites to `log_legacy!` during migration. Maintainer
  rendering keeps only their time, component and `<unstructured line>`.
  Track removal in [temporary measures](../temporary-measures.md#legacy-log-lines-during-typed-migration).
- Migrate every legacy engine and gateway site, then remove the legacy macro.
  CLI and other small crates stay outside this change; their output never
  reaches a bundle.
- Compile-pass and compile-fail tests enforce classification. Marker tests
  prove private values and legacy lines never reach maintainer output.

## Alternatives considered

- **Runtime shape guessing.** Ordinary text can resemble an identifier or
  kind name. A false positive would expose conversations silently.
- **Masking tokens and secrets alone.** Cannot recognize ordinary conversation text.
- **Treat all values as shareable unless annotated.** Omissions leak silently.
- **Treat all values as private by default.** Prevents useful metadata without
  requiring authors to decide what a value means.
- **A manual logging convention.** Review cannot reliably classify thousands
  of sites or stop future omissions. Compile failures enforce the boundary.
