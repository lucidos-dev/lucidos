# 0599: A relaunched shared SSE worker asks every port to introduce itself, and a page replaces a worker that stops answering

- **Status**: Accepted
- **Date**: 2026-10-29

## Context

ADR 0146 puts one `EventSource` per workspace in a `SharedWorker`. A page
attaches a port and says `hello`, which carries the stream URL. The worker
keeps that URL and its client set in module state.

WebKit relaunches a shared worker with a fresh global scope when the worker's
host process dies, and pages keep their ports. On the packaged macOS app this
left a window's stream dead for an hour while the page read as connected:

- Every send needed the 30 s safety refetch, because no confirming frame came.
- No stall and no error was logged.
- Each wake fetched `sse-worker.js` with no page load.

That fetch is the worker opening `new EventSource('')`. A `check` from a port
the fresh instance never met reached `restartSilentUpstream`, which opened the
empty stream URL. An empty URL resolves to the worker's own script. The request
failed, the worker told its empty client set, and it never retried.

Nothing on the page could notice. A shared stream has no page-side clock, and
`revalidate()` posted `check` and expected nothing back.

## Decision

- The worker posts `introduce` to every port it takes on. It posts it again
  in reply to a `check` or a `restart` from a port it has no `hello` from.
  That port also gets `error`, and no stream opens.
- The worker never opens an upstream without a stream URL.
- A page takes an `introduce` that arrives first on its link as answered by
  the attach `hello`. On any later one it reports a `worker-restarted` drop and
  says `hello` again. A link an older worker served therefore still repairs
  when that worker relaunches as a newer one.
- The worker answers every `check` with `checked`, and every `restart` with the
  `error` of its drop. Once any worker on the stream has sent `introduce`, a
  page waits `WORKER_ANSWER_MS` (5 s) for any message after either. On silence
  it reports `worker-unresponsive`, says `bye`, and attaches a new
  `SharedWorker`. A timer that fires a whole window late asks again instead:
  the page was frozen, and the answer may be queued behind the timer.
- The shell logs both drops as `[Client/event-stream]` lines, as it logs a
  stall.

## Rationale

The worker cannot know a port's stream URL until the port says so, and only
the page knows it ever said so. So the worker asks, and the page decides. An
`introduce` at attach costs one message per port, and it repairs a relaunch the
moment it happens, even on a page that never wakes.

The answer timer covers the other loss: a worker the browser dropped without a
relaunch. Any message counts as the answer, so a busy stream needs nothing
extra.

Both drops take the existing path. The page's next `open` runs the resync
armed by the drop, which is the only catch-up a stream gap has (ADR 0514).

## Consequences

- A relaunch costs one reopen and one resync, with a log line saying why.
- A worker that predates this protocol never sends `introduce`. A page that
  meets one sets no answer timer, so a newer page beside an older worker
  behaves exactly as before. Other windows can keep such a worker alive across
  a reload.
- An older page ignores `introduce` and `checked`. If a newer worker meets an
  older page's port with no `hello`, the page gets `error`. It then reads as
  disconnected rather than connected, and a reload fixes it.
- A replacement worker that never runs, for example while its script answers
  503, is replaced again on the next `check` or `restart`.
- A worker that dies without a relaunch is caught on the next focus or wake,
  which sends the `check`, or by the send probe's `restart`. Until then a
  focused, idle page gets no frames. A worker heartbeat with a page-side clock
  would close that gap. The maintainer chose to wait for the
  `worker-unresponsive` lines to show whether it happens
  (`docs/plans/2026-10-29-locate-where-event-stream-frames-are-lost.md`,
  Phase 3).

## Alternatives considered

- **Carry the stream URL on every `check`, so a fresh worker can adopt the
  port.** It repairs only on a wake, and it duplicates `hello`. Rejected for
  `introduce`, which repairs at once with one message.
- **Fall back to a direct `EventSource` when the worker stops answering.** The
  transport would change kind mid-stream: `ownsReconnect` flips, and the shell's
  retry rules with it. A new `SharedWorker` keeps one transport. Rejected.
- **Version the worker URL per build.** A new build would get a new worker,
  but a relaunch keeps the same URL, so it fixes nothing here. Rejected.
