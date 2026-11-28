# 0796: The voice module resolves the talker once per call in one place

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0148](0148-voice-is-a-mode-of-a-thread.md), [0149](0149-a-rented-talker-beside-the-agent-we-own.md), [0181](0181-a-live-talker-has-no-turns-and-no-tools.md), [0107](0107-aux-purpose-owns-its-preference-and-deadline.md)

## Context

`crates/lucidos-engine/src/voice/build.rs` decides which talker a workspace
calls. It works from the workspace's configured preference and credential.

Two talker protocols exist, Realtime and the Live API. They share a key and
nothing else.

This record restates the design comments of
`crates/lucidos-engine/src/voice/build.rs`, written before the comment strip
removes them. Nobody checked it line by line against the code.

## Decision

- One place resolves the talker. The socket handler never reads a preference or
  a credential itself.
- The result is `Ok(provider)` or a sentence saying why there is no provider.
- Resolution happens per call, not once at boot.
- The protocol is chosen from the model id by prefix. An id beginning
  `gpt-live-` speaks the Live API. Every other id speaks Realtime.
- The id decides the protocol here and nowhere above. Both protocols answer the
  same seam, so no caller learns which one it got.
- The transcription model is read directly. It does not go through a
  `ContextPurpose`.

## Rationale

A user who stores an OpenAI key in Settings and then presses the microphone
should be talking. They should not have to restart the engine, so nothing is
held from boot.

The family of ids grows without us. A prefix means an unlisted `gpt-live-*` id
belongs to the Live API by construction. A fixed list would not.

Everything else is Realtime, which is what an unknown id was before Live
existed.

A `ContextPurpose` pairs one purpose with one budget and one capture. The
transcription model makes no HTTP call of its own. It is a field in the
socket's opening frame.

## Consequences

- A key stored in Settings takes effect on the next call with no restart.
- The socket handler stays free of preference and credential reads.
- A new `gpt-live-*` id works with no code change.
- Code above `build.rs` never branches on the protocol.
- The transcription model has no purpose entry, so no deadline exists that
  nothing enforces.
- The transcription model adds no wire enum variant that nobody reads.

## Alternatives considered

- Holding the provider from boot. Rejected because a newly stored key would
  then need an engine restart.
- A list of Live model ids. Rejected because the family grows without us and an
  unlisted id would be misrouted.
- A `ContextPurpose` for the transcription model. Rejected because it would add
  a deadline nothing enforces and a wire enum variant nobody reads.
