# 0084: Volatile values never enter the cached system tier

- **Status**: Accepted
- **Date**: 2026-08-17

## Context

Anthropic caches by prefix, in the order `tools` then `system` then `messages`.
A `cache_control` marker on the last block of a tier caches everything before
it. Lucidos places three markers (`llm/anthropic_wire.rs`), and the fourth of
the four Anthropic allows is deliberately unused.

The chat system block carried `CURRENT TIME` near its front, built from
`Utc::now()`. A wire probe (`LUCIDOS_CACHE_PROBE=1`, `llm/cache_probe.rs`)
measured what that cost. On an in-TTL turn boundary the tools tier survived
while the ~22.9k-token system tier was rewritten: `cache_read=27145`,
`cache_creation=45824`, with `system_bytes` constant and only `system_hash`
moving.

The whole turn-boundary write is $0.4415, and the clock is one of three causes,
so this ADR removes roughly a third of it. The other two are untouched: the
trim rotation at messages index 0 (about a half), and unmatched tools prefix
residue of ~9,200 tokens (about a seventh). That rotation was confirmed
changing at 94.3% of 1,115 boundaries.

The clock's own share is about **$0.13 per boundary** on Opus, worth about
**$148 over 30 days**. That is the ~22,900-token system tier moving from the
write rate to the read rate, ($6.25 - $0.50)/MTok x 22,900. The 30-day figure
covers the 919 boundaries measured as `global_warm`, 65.1% of 1,412. Every
figure here is measured in the dev workspace artifact
`data/artifacts/context-economics-investigation.md`, over 19,254
`ContextCaptured` rows and 94 paired cache-probe wire lines in 30 days.

A second probe showed the cache is keyed on prefix bytes scoped to the API key,
not on a conversation: a brand-new thread read 27,145 tokens on its first call.
Two unrelated threads both presented 58,854 system bytes with differing hashes.
The clock alone kept a workspace-level block from being shared thread to thread.

## Decision

Two rules, both narrower than "keep the prompt small".

1. **The system block holds nothing that varies per turn or per thread.** It is
   a function of workspace state and preferences, and of nothing else.
2. **Anything volatile that reaches the message array is derived from persisted
   state**, never from a wall clock. For the clock that means the `created`
   stamp of the newest event already on the thread.

`engine::chat::process::turn_clock` owns both halves: `timezone_section` takes
no timestamp, and `current_time_block` takes only `turn_started_at`.

## Rationale

The second rule is what makes the first one worth anything. The message array is
rebuilt from events on every request, and the last breakpoint advances to the
newest last message each round. So a trailing block synthesized from
`Utc::now()` is uncached on the round that adds it, then lands *inside* the
cached prefix on the next one. Rebuild it from a moved clock and the prefix
changes: the miss relocates from the system tier to the message tier and the
probe still reports one, just in a different segment.

Deriving from persisted state removes that by construction. The clock is
`MAX(created)` over the thread's events, read once at turn setup, so it is
fixed for every round of the turn.

**Not the turn anchor's own `created`**, which was the first shape and is
wrong. The anchor answers which exchange the turn's events group under, and the
two part company on the answer-driven resume: `ChatResumeAnchor::ExistingTurn`
re-uses the interrupted turn's `request_event_id` on purpose, so a question
answered the next morning would have told the agent it was yesterday. The
newest event is the answer that woke the thread.

`MAX(created)` rather than the newest row by `sequence`. A
`replay_historical_event` backfill inserts a backdated row at a high sequence,
and a clock must not run backwards because one landed.

Pinning to persisted state costs no freshness. The system prompt was already
built once per turn, not per round: the probe found all 12 rounds of one turn
sharing a single `system_hash`.

## Consequences

- The agent reads the time at the END of its context rather than the top, in a
  `[CURRENT TIME]` block after the request line. The system prose points at it.
- The DST offset moved with the reading. Left behind it would be the one
  clock-derived value in a tier whose guarantee is that it holds none.
- One indexed aggregate per chat turn resolves the clock. An empty or
  unreadable thread falls back to the wall clock and logs. A turn with no clock
  is worse than one turn's cache miss.
- Adding a per-turn or per-thread value to the system prompt now breaks
  `two_threads_in_one_workspace_share_the_system_block`. That is the intended
  gate, and equal byte counts are not proof of equal content, so it asserts the
  hash.
- The trigger addendum stays where it is. It is thread-shaped, and it is already
  appended after every unconditional section precisely so the shared prefix
  survives ahead of it.

## Alternatives considered

**A fourth `cache_control` breakpoint, after the clock.** Rejected: 3 of
Anthropic's 4 are in use and the fourth is reserved. It would also buy nothing
here, since the trailing position is uncached on the round that writes it either
way.

**Keep `Utc::now()` and simply append it to the last message.** Rejected: this
is the trap above. It converts a system-tier miss into a message-tier miss and
reads as a fix on every metric except the one that matters.

**Return `created` from `EmitResult` instead of a lookup.** Rejected twice
over. The common path is `PreEmittedOrigin::Message`, the chat API boundary's
emit-before-ack, which never holds the timestamp. And the value wanted is not
the anchor's stamp at all, per the resume case above.

**A timestamp on every history line.** Rejected: history is a stringified blob
inside the user message, rebuilt per turn. Per-line stamps would grow the
message tier and buy no cache stability, since that blob is rewritten at every
boundary regardless.

**Drop the clock entirely and let the agent call a tool for it.** Rejected: a
round trip for something the prompt can state, on every turn that reasons about
"this week" or "since Monday".
