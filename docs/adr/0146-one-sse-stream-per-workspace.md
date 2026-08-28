# 0146: One SSE stream per workspace, held by a SharedWorker; presence means an active shell

- **Status**: Accepted
- **Date**: 2026-08-28

## Context

Every JS realm opened its own `EventSource` to `GET /api/v1/events`. The shell
opened one per tab. Each app iframe opened another, usually without asking:
`lucidos.ui.watchPreferences()` calls `sse.connect()` for you. Each app popped
out through *Open in new tab* is a full document and opened one more.

So connection count scaled with app count. A user should be able to open as many
apps as they like without loading the system more.

Reconnaissance found a second thing, in the code this touches. The push fan-out
waits for one pong per open SSE connection (`expected_pong_count`,
`scheduler/push.rs`), but **only the shell pongs**. There is no `presence-pong`
anywhere in `packages/lucidos-sdk/`, so an app iframe held a connection and
never answered:

| Documents | `sse_connections` | Pongs | Result |
|---|---|---|---|
| Shell alone | 1 | 1 | Decides at once |
| Shell + 2 apps | 3 | 1 | Waits the full 2000 ms deadline, every time |

A naive collapse would have broken it the other way. Put three shell tabs behind
one connection and the engine settles on the first pong. A background tab could
then decide the push while a foreground one was still answering.

## Decision

**One SSE connection per workspace, per browser profile, held by a
`SharedWorker`.** Every document of that workspace attaches to it. The holder
relays frames verbatim and ORs its documents' `PresencePong` answers into the
single pong its connection owes. Where `SharedWorker` is missing, a document
opens its own `EventSource` exactly as before.

**Presence means an active shell.** `is_active` is `isPageActive()` AND no app
fullscreen, native or pseudo. A fullscreen app therefore takes the OS push.

## Rationale

**The holder is not a document, and that is the whole point.** A background tab
can be frozen while it still holds a lock. Electing one tab as leader therefore
starves foreground followers, with no error and no way to notice. A
`SharedWorker` is not frozen that way and exits when its last port goes, which
is exactly when the stream should end.

**Aggregating the pong keeps an existing engine invariant true rather than
patching around it.** The engine already expects one pong per connection. The OR
one layer down restores that, so the engine needs no change. It is also the same
OR the engine already applies across tabs on one device.

**Presence answers "can we reach this person without pushing?", and only the
shell can show a toast.** A reader deep in a fullscreen app is not reading the
shell. It also makes a fullscreen app agree with a popped-out app window: the
two are identical from where the user sits, and a windowed one has always taken
the push. That they were opposite was an accident, not a choice.

## Consequences

What we keep:

- Opening apps no longer opens connections. The multiplier is gone.
- The 2000 ms push-decision stall with any app open is gone with it.
- The engine is untouched apart from one static route. A relayed frame is
  byte-identical to a direct one, so nothing downstream can tell them apart.
- Reconnect logic lands in one place per workspace instead of once per document.

What we give up:

- **Two transports to maintain, permanently.** The direct path is today's code,
  so it costs nothing to build, but it does have to keep working.
- **A fullscreen app now interrupts with an OS notification** where it used to
  show a quiet in-app toast. That is the trade, taken deliberately.
- **A committed build artifact.** The worker bundle is checked in so
  `cargo build` needs no prior npm run, guarded by a staleness test.
- **Debugging is harder.** A `SharedWorker` has no easy devtools view, and a
  bug in it affects every document of the workspace at once.

## Alternatives considered

**A `BroadcastChannel` leader, elected with Web Locks.** No worker, works in
every browser we support, and one tab holds the stream for the rest. Rejected on
the frozen-leader trap: a browser freezes a background tab without closing it,
so the lock is still held and the elected holder relays nothing. The foreground
window reads as connected and silently goes stale, with no error to show and
nothing to click. Detecting that needs a liveness heartbeat and a takeover
protocol. That is a lot of machinery to buy back a property the worker has for
free.

**The service worker, which is already registered at `SCOPE_PATH`.** It covers
app iframes for free and needs no new registration. Rejected outright: browsers
terminate an idle service worker, an SSE stream is not a reliable keepalive, and
that worker currently owns push. Wedging it costs notifications, which is a
strictly worse failure than the one being fixed.

**Host-to-iframe `postMessage` only.** The narrowest version: the shell relays
to its own app iframes over the bridge that already exists, with no worker and
no new primitive. It is genuinely safe, because a shell and its iframe suspend
together, so the holder can never sleep while a follower is awake. Rejected as
too narrow: it does nothing for a second tab, and nothing for a popped-out app
window, which is the case that actually multiplies.

**Declare browsers without `SharedWorker` unsupported.** Considered because it
would drop one of the two transports. Rejected because it drops nothing: the
fallback IS the existing code, kept by not deleting it. Removing it would leave
Chromium on Android with no event stream at all. The gap also lands where it
costs least, since the phone UI holds at most a shell plus one app-ui overlay.

**Give apps a presence voice instead.** The other way to make fullscreen and
popped-out agree: an SDK heartbeat plus an automatic notification surface, so
suppressing a push leaves the reader something. It is the more generous answer
and it is not ruled out. It is also a much larger change, and narrowing what
counts as active reaches the same consistency now.

**Leave presence alone and ship only the transport.** Coherent, and it keeps the
diff to one concern. Rejected because the two land in the same function: Phase 3
was already rewriting the pong path, so the marginal cost was a predicate and
its tests.
