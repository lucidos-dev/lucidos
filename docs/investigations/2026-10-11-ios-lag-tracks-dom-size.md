# Investigation: iOS PWA lag after the off-screen pause

The iOS PWA lagged again on 2026-10-11 in the evening, a few hours after the
off-screen pause shipped. That fix pauses every animation in a pane that is off
screen (`.mobile-swipe-pane[data-offscreen]` in `styles/mobile.css`). This note
asks whether the fix was lost, and what still lags.

Status: open. The cause is identified; no fix has been built yet.

Source: the iPhone's `[Client/perf]` and `[Client/page-health]` lines in
`~/.lucidos/gateway/gateway.log`. How to read them: `docs/client-logs.md`.

## The off-screen pause is in place, and it worked

The attribute, the rule and their tests are all on `main`. The pane-strip
rewrite that landed an hour later (ADR 0427) kept `data-offscreen`.

The logs show the effect. A composer `keydown` measures how long the main
thread was busy before the key reached our handler (`inputMs`).

| Window | Slow keydowns | Median wait | Worst wait | Worst total |
|---|---|---|---|---|
| Before the fix (13:20 to 15:55) | 15 | 0.59 s | 1.60 s | 2.40 s |
| After the fix (17:00 to 19:05) | 6 | 0.31 s | 0.57 s | 0.58 s |

So typing no longer waits seconds behind off-screen animation. The lag that
remains is a different cause.

## What still lags

1. **Slow keystrokes cost about 140 ms, almost all of it render.** A slow
   `input` on the composer shows `handlerMs` of 5 and `renderMs` of 138. That
   is style, layout and paint, not our JavaScript.
2. **The main thread stalls for 0.15 to 0.5 s every few tens of seconds.**
   With a very long thread open, a stall lasts 1.4 to 2.6 s. A key pressed
   during a stall waits for it to end.
3. **Renders are slow while the page is big.** A send into a near-empty thread
   re-rendered in 335 to 470 ms at 19:00, against a usual 40 to 60 ms. The
   page held 8,000 to 9,400 DOM nodes then.

## The lag tracks DOM size

The page-health samples, grouped by the `nodes` count, after removing
suspended windows:

| DOM nodes | Samples | Mean `lagMaxMs` | Windows with a stall over 300 ms |
|---|---|---|---|
| under 3,000 | 475 | 103 | 9% |
| 3,000 to 10,000 | 88 | 243 | 17% |
| over 10,000 | 67 | 524 | 52% |

The page reached 21,789 nodes at 17:50, with a coding-agent thread of 24,512
events open. That is the window with the 1.4 to 2.6 s stalls.

## Why the DOM grows

The phone mounts every pane at once (`MobileSwipeContainer`), so the DOM holds
the drawer, the transcript and the content pane together. Two parts grow without
bound.

**The thread drawer draws every Current thread.** `ThreadDrawer.tsx` pages only
the Archive section. Every thread in Current renders a full row, with its own
icons and badges. With about 400 threads in Current, the page held 7,500 to
9,400 nodes while showing a near-empty thread.

The user archived most of them at 19:10, and the next samples confirm it:

| Window | DOM nodes | Worst `lagMaxMs` |
|---|---|---|
| 19:03 to 19:10, about 400 in Current | 5,200 to 9,400 | 321 |
| 19:10 to 19:16, most archived | 900 to 2,900 | 89 |

**The transcript grows too:**

- **The transcript window only grows.** `threadWindow.ts` renders a tail of the
  thread and adds turns as the reader scrolls up. Nothing removes turns again.
- **It outlives the visit.** `renderFloorByThread` in `ThreadView.tsx` is module
  scoped, so a thread reopened in the same session draws everything it drew
  before.
- **Some paths draw the whole thread.** Scroll to top
  (`scrollToTopNeedsRenderAll`) stores a render-all, and a reading position or a
  deep link far back grows the window until it reaches its turn.

Every style or layout pass then walks that tree. On an iPhone, a few thousand
extra nodes is the difference between a 40 ms and a 400 ms frame.

## Ruled out

- **The gateway's 2 s fan-out.** Every engine gets `GET /notifications` and
  `GET /backup/last-successful` every 2 s. That is the gateway's supervise pass
  (`SUPERVISE_INTERVAL`), not a client. Each answer takes 1 to 15 ms.
- **Pane bounces.** Bursts of pane `navigation` lines are swipes inside one 10 s
  flush, not a loop.
- **Root custom properties.** `--app-height` and `--keyboard-band` change only
  on a viewport resize, not per keystroke.

## Where a fix would go

Bound the DOM, not the per-keystroke work:

- **Window the drawer's Current section**, as Archive already is. A long
  Current list is ordinary use, and it costs every frame on every pane.
- **Window the transcript at both ends.** Drop turns far from the viewport, and stop a render-all
  from staying pinned for the session. It touches three settled decisions, so
  it needs a plan: ADR 0081 (no skeleton hold), ADR 0152 (restore by a turn's
  top edge) and ADR 0234 (the reading chase).
- **Skip the rendering of off-screen turns** with `content-visibility: auto`
  on each turn. This is cheaper, but it changes `scrollHeight` while turns
  render, which the scroll anchoring and deep links rely on.
