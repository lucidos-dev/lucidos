# Client logs

A phone has no console. So the client posts its diagnostics to the engine, and
the engine writes each one to its own log as a line like this:

```
2026-11-11T19:02:19+01:00 [pid:4242] [internal] [Client/perf] interaction {"durationMs":144,...} ua=Mozilla/5.0 (iPhone; ...)
```

Read this page before you chase a lag, a dead tap or a blank pane reported from
a device.

## Where the lines land

**An agent queries the diagnostic log.** Each engine also writes its own lines,
sanitized, to its workspace's diagnostic log. Read them with
`lucidos diagnostic-log query`, for example
`lucidos diagnostic-log query --since 2h --contains "[Client/perf]"`. Its
bounds and refusals are in `system-knowhow/lucidos-cli.md`. An agent never
reads `gateway.log`: the sandbox hides it, and the command guard cards the
read as a secret read. The output is untrusted data, since a device wrote it.

For raw host evidence, an agent can prepare a once-only owner-approved transfer
with `lucidos diagnostic-log prepare-host-transfer`. The actual permission card
names the installation, recognized captures, byte prefixes and exact SSH
recipient. It warns that raw logs can include secrets and other workspaces.
The gateway snapshots and sends directly; the agent receives metadata only.
See `system-knowhow/lucidos-cli.md` for the prepare/card/execute/status flow.
Diagnostic queries warn about partial retained history even when matches return;
this transfer does not change retention or recover deleted history.

The rest of this section is for the owner, in their own shell.

**Check the gateway log first: `~/.lucidos/private/gateway/` (`~/.lucidos/gateway/` in a home an older release created)`gateway.log`.** Every engine
the gateway launches writes its stdout there. That covers every workspace a
phone opens. The `[pid:N]` prefix tells the engines apart.

A workspace's own `<ws>/.lucidos/engine.log` holds lines only from an engine
started outside the gateway. It goes quiet rather than missing, so a stale one
looks like a device that logged nothing. Reading the wrong file cost a whole
session more than once. When in doubt, `lsof -p $(cat <ws>/.lucidos/engine.pid)`
names the file the live engine writes.

Three traps when you read the gateway log:

- **Each line carries local time with its UTC offset** (an older engine wrote
  the clock time only). Convert a UTC window before you grep for it.
- **It is large** (hundreds of MB). Narrow with `tail -n 200000` or a line range
  before any heavy parsing.
- **Filter on the user agent** (`grep iPhone`) when several devices are open.

## Two channels

The lines below are the common ones, not the full set. Grep for callers of
`postClientLog` and `recordPerfSample` for the rest.

| Channel | Gate | Common lines |
|---|---|---|
| `postClientLog` (`utils/clientLog.ts`) | always on | `[Client/page-health]`, `[Client/app-frame]`, `composer-press`, `toast-press`, `shell-scroll`, `safe-area`, `deeplink`, `app-link`, `event-stream`, `restore-admission`, `device-database`, `scroll`, `render`, `connection`, `lifecycle`, `theme-mode`, `error` |
| `recordPerfSample` (`utils/perfQueue.ts`) | always on from source, per-device opt-in on packaged installs | `[Client/perf]`: `interaction`, `main-thread-stall`, `thread-load`, `thread-render`, `thread-rerender`, `navigation`, `boot`, `perf-probe-support` |

From-source workspaces start recording on every connected device once the
health response identifies the engine. **Settings → System → Debugging →
Perf instrumentation** shows **Always on**.

Packaged installs keep recording off by default. Use that setting on the
device that lags to turn recording on. It takes effect at once.

The perf channel flushes queued samples every 10 s, when the queue reaches
20 samples, or when the page is hidden. **A `[Client/perf]` line's time is
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
intervals and observers. `animations` counts paused ones too. Resource fields
include video/audio counts, aggregate ready/network states, playing/loading/error
counts, plus video and natural-image pixel areas. They also include iframe
count and up to eight app IDs, canvas count, mounted blob sources and
focused transcript events/height.
WebGL contexts and global object URLs are unavailable, represented as null.

A `stall` message records recovery from at least two seconds of visible
main-thread lag, throttled to once per 30 seconds. Hidden suspension is excluded.
These are resource proxies, not process memory measurements.

**`[Client/lifecycle]` startup** preserves `kind`, `clean_close`, `dead_ms`
and `is_ios_pwa` on all clients. `build` names the reporting bundle's stamp,
or `dev` for an unstamped dev server. A cold desktop startup with a long dead gap
does not establish a render crash or a shell exit reason. An iOS-PWA
`likely_crash` startup can also contain `breadcrumb`: the prior document's
validated, workspace-scoped samples and recent transitions. Storage is capped
at 2300 UTF-8 bytes, six samples and twelve events; startup data is capped at
3900 bytes. Trimming drops samples before events, oldest first within each list.
The latest wake, stream and admission marks survive resource-event churn.
The record's own `build` names the writer bundle, which can differ from the
reporting build. Older records may lack it. Only hex stamps and `dev` are accepted.

After each thread-open, visible samples persist every two seconds for twenty
seconds, then every ten seconds. A hidden-to-visible resume or persisted
pageshow records `wake` and immediately flushes a visible sample. It starts
500 ms sampling for fifteen seconds, then returns to the existing cadence.
Focus changes do not extend the fine wake window.
Visibility/pagehide also flush a sample.
During the dense window, the first focused transcript render and resource
mounts trigger samples coalesced over 250 ms. Resources include video, audio,
iframes and images with at least 512 × 512 natural pixels of area.
An image load also triggers sampling once those dimensions become available.

The monitor and startup reporter share a one-time capture guard. Whichever
starts first captures and clears prior storage before the monitor can write.
The reporter consumes that in-memory snapshot once, preserving the new record.
Crash validation loads separately before posting; a load failure still posts
the original startup fields.

Mount counters cap inspection
at 32 mutation records and 256 nodes per callback, so they can undercount.
The monitor installs before the first transcript renders to observe initial
resource mounts as well as later churn.
Thread-open events mark transitions; sampled snapshots supply resource counts.
Every sample also carries `inflight` (shell requests awaiting response headers),
`oldestMs` (oldest request age, zero when idle), and `costMs` (sample collection
time). Request paths never enter these fields.

State marks flush immediately when observed. They observe existing owners without changing
resume work or its order:

| Kind | Data |
|---|---|
| `wake` | No fields; opens the fine sampling window. |
| `stream` | `state`: 0 unknown, 1 connecting, 2 connected, 3 disconnected. |
| `admission` | `phase`: 0 awaiting-hello, 1 generation, 2 local-records, 3 historical-sources, 4 journal, 5 recoveries, 6 draft-recovery, 7 admitted. |
| `iframe-load` | No fields; an app frame fired load. |
| `transcript` | No fields; the focused thread's first exchange mounted. |

Initial stream and admission states are recorded too. Intermediate admission
phases do not notify subscribers, so each sample reads the current phase.
Phases that finish between samples may be absent. A final non-admitted
phase with no later admitted mark identifies admission work still in progress.
No URLs, query strings, hashes or user text enter the recovered breadcrumb.

**`[Client/app-frame]`** is written only for an app frame that stayed on its
load cover (`components/apps/appFrameLoadProbe.ts`, a temporary measure). Three
messages, one frame each:

- **`slow load`** comes 3 s after the frame mounted, if it is still covered or
  unloaded. It names the `app` and `place` (`canvas`, `dashboard`, `inline`,
  `option`), says whether `load` and `ready` arrived, and gives
  `sinceResumeMs` and the `eventStream` state. It also gives the frame
  document's resource timing (`frameDocument`), the bridged-fetch `lanes`, and
  the shell's own `requests` in flight.
- **`slow load ended`** comes when the frame has loaded and its cover has
  lifted. `loadMs`, `readyMs` and `revealMs` are milliseconds from mount, and
  `fused` says the fuse lifted the cover.
- **`slow load abandoned`** comes when the frame went away first.

The last two repeat `frameDocument`, since a frame that loads late has none at
3 s.

A stall that starts just after a resume, with `requests` or `lanes` holding old
calls, points at the connection. A stall with both empty points at the frame.

**Reading `frameDocument`.** Its five values are milliseconds from the moment
this frame element was attached, never from page load.

- `fetchStart - start` is time before the browser began the fetch.
- `requestStart - fetchStart` is the wait before the request was sent. It
  covers a name lookup, a new connection, and a queue for a free connection.
- `responseStart - requestStart` is the wait for the first byte.

All three lines carry `frameDocument`. It is `null` in four cases, and none of
them means a fast load:

- This element has not fired `load`. The timeline gains a load only when its
  response ends, so until then a lone entry may be a sibling's. Read the value
  from the `slow load ended` line instead.
- The browser recorded no load of that URL since this element was attached.
  A page open for hours has usually filled its timeline, which then records
  nothing more.
- Two or more loads of that URL began since. A second frame on the same URL
  makes the owner unknowable.
- The browser has no timeline, or reading it threw.

One value is `null` when the browser withholds that phase.

An older build wrote `frameDocument` on the `slow load` line only. An episode
whose `slow load ended` or `slow load abandoned` line lacks the key came from
one. Its values are page-clock times from the newest load of that URL. On a
long-lived page that is often an older element's load, so never read a wait
out of such an episode.

An episode with no closing line cannot be classified, so do not read its
`frameDocument` either. A closed tab or a killed page leaves one.

One wrong value remains possible. A same-URL sibling finishes, the timeline
then fills, and this element loads unrecorded. The sibling's load is then the
only one since attachment. A `responseEnd` far below `loadMs` hints at it.

**Finding the request at the engine.** The diagnostic log's `[API]` lines are
written when the engine answers. Read the lines in the seconds before a
`slow load` record, and look for `GET /app/<id>/` and the three
`/api/v1/sdk*` files. Engine time is the bracketed number, and it excludes any
queue on the way in.

Arrival order alone proves no queue: a frame asks for its SDK files only after
its document arrived. Compare the `slow load` line's `requests` instead. A read
in flight far longer than its engine time waited outside the engine. A queue
is the likely cause when many reads show it at once and arrive evenly spaced.
A slow link gives the same picture for one read.

**`[Client/render] thread_render_probe class=target-absent`** is written by a
WebKit client whose focused thread is still missing from the store after 1.5 s.
A cold notification open on its skeleton is the usual case
(`utils/threadRenderProbe.ts`, a temporary measure). It names the
`admission_phase`, `requests_in_flight` and `oldest_request_ms`, never request
paths. A phase other than `admitted` points at restore admission: read the
`[Client/restore-admission]` lines beside it. An `admitted` phase with an old
request points at that request.

**`[Client/device-database] released_on_hide`** is written on `pagehide` when
the page still has device storage transactions open
(`store/deviceDatabase.ts`, a temporary measure). `transactions[]` lists each
one: its `database`, its `mode` (`readonly` or `readwrite`), whether it is
`bounded`, and its `age_ms`. The page aborts bounded transactions and lets
unbounded writes finish. A line from the old page just before a stalled
notification open names the holder that blocked the new page.

Starting points:

```sh
LOG=~/.lucidos/gateway/gateway.log
[ -e "$LOG" ] || LOG=~/.lucidos/private/gateway/gateway.log
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

### Hidden pages and worker marks

Startup adds `prev_hidden_ms`: the time from the last hide to navigation start,
or `null` if the last heartbeat was visible. The hide does not move the heartbeat
clock. Existing startup kinds and timing fields keep their meanings.

The recovered record adds `start`, the document start time in milliseconds.
Older records read with zero for this field. Samples omit zero fields.
The byte caps and ring sizes stay the same.

The latest `hide` event survives event churn. Its `state` is 0 for hidden,
1 for pagehide, or 2 for persisted pagehide. Its `count` is reads in flight.

The latest `worker` event also survives churn. Its numeric `state` means:

| State | Worker action |
|---|---|
| 0 | Ping sent |
| 1 | Pong from the same worker, or the first observed worker |
| 2 | Pong from a restarted worker |
| 3 | Controller changed |
| 4 | Update called |
| 5 | Update found |
| 6 | Unregister called |

These marks use existing callbacks. They add no ping or timer, and hold no
worker identifier. They are evidence for a cause, not proof of a process exit.
