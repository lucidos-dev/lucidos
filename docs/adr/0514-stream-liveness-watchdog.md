# 0514: An event stream that goes quiet is reopened by a client watchdog, fed by a named keepalive event

- **Status**: Accepted
- **Date**: 2026-10-18

## Context

An `EventSource` reports a dead connection only when the browser notices. A
connection can die with no signal: a laptop sleeps, a proxy drops the socket, or
a network changes. The stream then looks open and delivers nothing. Frames
emitted in that gap reach no tab, and the UI goes stale until a reload.

The engine already wrote an SSE comment line as a keepalive. A comment is
invisible to every client API, so nothing could tell a quiet stream from a dead
one.

## Decision

The engine sends the keepalive as a named event, `keepalive`, every 30 seconds.
Each client runs a *stream watchdog* (see `docs/glossary.md`). It resets on any
frame or keepalive. After `STREAM_STALL_AFTER_MS` of silence (two intervals plus
5 seconds) it reports a stalled drop and reopens the stream.

The shared worker runs the same clock on its upstream. When a port says `hello`
and the upstream has been silent for `LATE_JOIN_STALL_AFTER_MS` (one interval
plus 5 seconds), the worker restarts the upstream at once.

A stalled drop takes the existing reopen path. The resync on open
(`needsResyncOnOpen` in `thread-sync.ts`) is the only catch-up. The page logs
each stall as a `[Client/event-stream]` line with the silence length and the
transport.

## Rationale

The fix sits at the one layer every consumer shares, so no feature needs to know
the stream can stall. A named event keeps `onmessage` consumers unchanged: apps
see no new frame. Two intervals of silence means one missed keepalive is
tolerated, and the slack covers timer jitter.

## Consequences

- Silence over about 65 seconds costs one reopen, then a resync. No frame is
  replayed.
- The keepalive interval is now a client contract. Lengthening it raises
  `STREAM_STALL_AFTER_MS` with it, because the constant is generated from the
  engine value.
- A tab that sleeps trips the watchdog on wake. The reopen is cheap, and the
  resync is what the wake path ran anyway.

## Alternatives considered

- **A refetch in each feature.** Every surface would poll or re-read on its own
  signal. That repeats the bug in each new surface, and the frontend rules forbid
  polling. Rejected.
- **A resume cursor with `Last-Event-ID` replay.** It would close the gap with no
  resync, but the engine would need a bounded replay buffer per client and
  event ids on every frame. The resync on open already covers the same ground
  with state the client can read. Rejected as cost without a gain.
- **Keep the SSE comment and watch for bytes.** `EventSource` and the worker's
  reader expose no comment lines, so the client cannot see them. Rejected.
