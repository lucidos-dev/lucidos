# Client logs

A phone has no console. So the client posts its diagnostics to the engine, and
the engine writes each one to its own log as a line like this:

```
19:02:19 [pid:4242] [internal] [Client/perf] interaction {"durationMs":144,...} ua=Mozilla/5.0 (iPhone; ...)
```

Read this page before you chase a lag, a dead tap or a blank pane reported from
a device.

## Where the lines land

**Check the gateway log first: `~/.lucidos/gateway/gateway.log`.** Every engine
the gateway launches writes its stdout there. That covers every workspace a
phone opens. The `[pid:N]` prefix tells the engines apart.

A workspace's own `<ws>/.lucidos/engine.log` holds lines only from an engine
started outside the gateway. It goes quiet rather than missing, so a stale one
looks like a device that logged nothing. Reading the wrong file cost a whole
session more than once. When in doubt, `lsof -p $(cat <ws>/.lucidos/engine.pid)`
names the file the live engine writes.

Three traps when you read the gateway log:

- **No date, only the clock time.** Find a day boundary where the time jumps
  back, or work from line numbers (`grep -n`).
- **It is large** (hundreds of MB). Narrow with `tail -n 200000` or a line range
  before any heavy parsing.
- **Filter on the user agent** (`grep iPhone`) when several devices are open.

## Two channels

The lines below are the common ones, not the full set. Grep for callers of
`postClientLog` and `recordPerfSample` for the rest.

| Channel | Gate | Common lines |
|---|---|---|
| `postClientLog` (`utils/clientLog.ts`) | always on | `[Client/page-health]`, `[Client/app-frame]`, `composer-press`, `toast-press`, `shell-scroll`, `safe-area`, `deeplink`, `event-stream`, `scroll`, `render`, `connection`, `lifecycle`, `theme-mode`, `error` |
| `recordPerfSample` (`utils/perfQueue.ts`) | off by default, per device | `[Client/perf]`: `interaction`, `main-thread-stall`, `thread-load`, `thread-render`, `thread-rerender`, `navigation`, `boot`, `perf-probe-support` |

Switch the perf channel on from **Settings → System → Debugging → Perf
instrumentation**, on the device that lags. It takes effect at once.

The perf channel sends in batches every 10 s. **A `[Client/perf]` line's time is
the flush time**, so events up to 10 s apart share one timestamp.

## Reading a lag

**`interaction`** is one slow input event (Event Timing), logged at 100 ms and
over. Which part is large is the diagnosis:

| Field | Means | Large when |
|---|---|---|
| `inputMs` | wait before our handler ran | the main thread was already busy |
| `handlerMs` | our own JavaScript | a handler or a synchronous render is slow |
| `renderMs` | handler end to the next frame | style, layout or paint is slow |

Event Timing rounds each duration to 8 ms.

**`main-thread-stall`** is a timer that fired late (`utils/mainThreadStall.ts`).
WebKit offers no Long Animation Frames and no long tasks, so this is the only
stall signal a phone gives. `perf-probe-support` says which observers the
browser took.

**`[Client/page-health]`** is one sample every 30 s from a touch device while
visible (`utils/pageHealthLog.ts`, a temporary measure). It carries `lagMaxMs`,
`lagMeanMs`, `fps`, `nodes` (DOM size), `animations`, and the live listeners,
intervals and observers. `animations` counts paused ones too.

**`[Client/app-frame]`** is written only for an app frame that stayed on its
load cover (`components/apps/appFrameLoadProbe.ts`, a temporary measure). Three
messages, one frame each:

- **`slow load`** comes 3 s after the frame mounted, if it is still covered or
  unloaded. It names the `app` and `place` (`canvas`, `dashboard`, `inline`,
  `shelf`, `option`), says whether `load` and `ready` arrived, and gives
  `sinceResumeMs` and the `eventStream` state. It also gives the frame
  document's resource timing (`frameDocument`, `null` when the browser hides
  it), the bridged-fetch `lanes`, and the shell's own `requests` in flight.
- **`slow load ended`** comes when the frame has loaded and its cover has
  lifted. `loadMs`, `readyMs` and `revealMs` are milliseconds from mount, and
  `fused` says the fuse lifted the cover.
- **`slow load abandoned`** comes when the frame went away first.

A stall that starts just after a resume, with `requests` or `lanes` holding old
calls, points at the connection. A stall with both empty points at the frame.

Starting points:

```sh
LOG=~/.lucidos/gateway/gateway.log
# Slow taps and keystrokes on the phone, newest last
grep iPhone "$LOG" | grep 'Client/perf\] interaction' | tail -40
# Stalls and big renders
grep iPhone "$LOG" | grep -E 'main-thread-stall|thread-(re)?render' | tail -40
# The health trend for the current session
grep iPhone "$LOG" | grep 'Client/page-health' | tail -20
# Widget tiles that sat on their cover
grep iPhone "$LOG" | grep 'Client/app-frame' | tail -20
```

**Ignore the `GET /notifications` and `GET /backup/last-successful` pairs**
every 2 s on every engine. They are the gateway's own health pass, which reads
each running engine's unread count and backup line. No client sends them.

A worked reading: `docs/investigations/2026-10-11-ios-lag-tracks-dom-size.md`.
