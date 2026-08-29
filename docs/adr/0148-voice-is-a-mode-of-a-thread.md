# 0148: Voice is a mode of an ordinary chat thread, never a kind of thread

- **Status**: Accepted
- **Date**: 2026-08-28

## Context

Lucidos is adding voice. The 2026-06-01 voice note proposed a new thread
`source = 'voice'` beside `'chat'` and `'trigger'`, and treated a voice session
as its own kind of thread.

A *thread* already carries a `source` and an `EventChannel`. `source` says how
the thread began. `EventChannel` has three variants: `Chat`, `ClaudeCode` and
`Trigger`. Both are read by projections, filters, the timeline and triggers.

## Decision

Voice is a mode of a *chat thread*. A voice session adds no `source` value and
no `EventChannel` variant. A thread's `source` stays `chat` across a whole
session, and the composer stays live, so speech and typing interleave in one
transcript.

The microphone control sits in every prompt input, the compose view included. A
session started with no thread open creates a chat thread through the same path
typing uses. Voice gets no entry point of its own.

## Rationale

The user does not start a different kind of conversation by speaking. They speak
in the conversation they already have. A separate source would say otherwise in
every projection that reads it.

The cost of the fourth variant is paid everywhere, not once. Every filter and
every `match` over the enum would grow a case. Each one is a place a voice
thread can be forgotten. A thread that is a chat thread by construction cannot
be forgotten by any of them.

Both ChatGPT and Hermes Agent model voice this way. Neither offers a voice
conversation you cannot then type into.

Keeping the composer live falls out of the same choice. If voice were a kind of
thread, a mixed transcript would be a special case. As a mode it is the default.

## Consequences

- `thread_summaries.source` reads `chat` for a voice thread, so nothing
  downstream needs teaching.
- Voice inherits thread titling, thread listing, search and triggers for free.
- A voice session is not addressable as a thing in its own right. Two new thread
  events mark its bounds instead.
- Nothing in the UI can filter for "my voice conversations". That is a real
  loss, and the events are the way back if it is ever wanted.

## Alternatives considered

**A fourth `source = 'voice'`.** The 2026-06-01 note's proposal. Rejected: it
divides threads by input method, which is not a division the user makes. It also
puts a per-variant obligation on every consumer of `source`, forever, in
exchange for a distinction only the transcript needs.

**A fourth `EventChannel::Voice`.** Rejected for the same reason, and it is
worse. `EventChannel` drives message grouping and channel filtering. A voice
turn would group apart from the typed turns beside it in one conversation.

**A boolean `voice_active` on the thread row.** Rejected: it is state that only
holds while a socket is open, so it violates engine statelessness. It would also
be wrong after any restart. The two session events carry the same information
without claiming to be current.
