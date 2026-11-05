# 0546: App bridge fetches share three slots, fairly across frames, and a /ui call skips the queue

- **Status**: Accepted
- **Date**: 2026-10-22

Plan: `docs/plans/2026-10-22-app-bridge-fetch-lanes.md`.

## Context

A "Review in thread" click in an app took about 15 s. The engine answered the
`POST /ui/navigate` in under a millisecond. The request waited in the browser,
behind about 120 background GitHub proxy calls the same app fired on open.

An isolated app frame cannot fetch the engine itself, so the shell makes every
call over the *app bridge* (ADR 0231). Those calls share the shell's HTTP/1.1
pool to the engine origin, about six connections. `bridgedFetch` had no limit.

## Decision

The host bridge runs at most three `fetch` calls at once, across all frames
(`BRIDGED_FETCH_SLOTS`). A free slot goes to the waiting frame with the fewest
calls running, ties by arrival. A `/ui/*` call skips the slots and dispatches at
once. A request whose `timeoutMs` deadline passes in the queue is rejected
unsent, and a sent one gets only the time left. A frame that unmounts loses its
queued calls.

## Rationale

**The host is the one place every frame's call passes.** Only the shell sees the
pool and the other frames. The SDK, inside one frame, sees neither.

**Three leaves the shell its own connections.** The pool is about six and the
shell's event stream holds one, so the shell keeps two.

**Fewest-running fairness stops a flood starving a widget.** A frame that fires
a new call on every completion cannot hold all three slots against a frame that
has none.

**A `/ui/*` call is what a click sends.** It is rare and short, so it cannot
flood, and a user feels its delay most.

**A deadline in the queue means the app gave up.** The SDK already told its
caller the call failed. Sending it later could land a write the app reported as
failed.

## Consequences

An app that fires a burst only queues its own work. Standalone app tabs fetch
directly and are untouched. Over a gateway that speaks HTTP/2 the slots cap one
shell's bridged parallelism at three, which costs little.

## Alternatives considered

- **Cap in the SDK.** Rejected: the frame cannot see the shell's pool or other
  frames, so ten frames would still send thirty calls.
- **A cap per frame only.** Rejected: it bounds one app, but many frames still
  fill the pool, and a click still waits behind its own frame's calls.
- **Strict round robin across frames.** Rejected: a frame that just filled every
  slot goes first again, ahead of a frame that has none running.
- **A per-app priority setting.** Rejected: it adds an app-visible API for what
  `/ui/*` already marks.
