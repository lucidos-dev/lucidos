# 0436: The first-paint JS set, entry plus shell plus their static imports, is budgeted at 1400 kB; lazy chunks get a 400 kB advisory; chunkSizeWarningLimit is off

- **Status**: Accepted
- **Date**: 2026-10-12

## Context

ADR 0288 split `crates/lucidos-app` into an entry chunk and a shell chunk. The
entry holds the data layer, the shell holds the UI, and the entry got a 600 kB
budget. ADR 0353 added a 90 % soft line and taught the dev build-watch to report
rather than fail. The shell chunk had no budget. It met a limit only by
accident: Vite's generic `chunkSizeWarningLimit` advisory, 600 kB for every
chunk, which the nightly clean build counts as a failure.

That accident did two wrong things at once. It held the shell to the entry's
number, which was never chosen for it. And it held every lazy chunk to a
first-paint number, though a lazy chunk never loads before first paint.

We measured before choosing. One build per day, from the shell split to the day
of this decision, all in kB of 1000 bytes, raw:

| Day | Entry | Shell | First-paint set | Set gzip |
|---|---|---|---|---|
| 09-26 | 492.8 | 364.2 | 975.4 | 313.1 |
| 10-03 | 519.8 | 496.6 | 1167.3 | 378.0 |
| 10-07 | 542.9 | 557.9 | 1255.4 | 405.8 |
| 10-11 | 529.9 | 643.3 | 1333.3 | 433.1 |
| 10-12 | 530.2 | 595.3 | 1285.3 | 418.1 |

The **first-paint set** is the entry chunk, the shell chunk, and every chunk
either reaches through static imports. On 10-12 that was five chunks: entry
530.2, vendor 29.7, shell 595.3, `useTooltip` 58.5, `marked` 71.6.

Two facts stand out. Entry and shell trade bytes: each entry trim moved code
into the shell (10-03: entry -75, shell +64). And the set grows steadily, about
24 kB a day raw and 8 kB gzipped. Only moving code behind a dynamic import
lowered it.

## Decision

1. **The budgeted unit is the first-paint set, in raw JavaScript bytes.** Its
   budget is **1400 kB**, with a soft line at **1260 kB** (90 %).
2. **The entry budget stays**: 600 kB, soft line 540 kB.
3. **Each lazy chunk gets an advisory at 400 kB.** It warns and never fails.
4. **`build.chunkSizeWarningLimit` is off** (`Infinity`). `vite/chunkBudget.ts`
   judges every chunk against the number for its class.

Both hard budgets behave as ADR 0353 set out. A single-shot build fails past the
budget. Every build warns past the soft line, naming the headroom left. The dev
build-watch reports instead of failing, records each budget in
`.build-watch/status.json`, and alerts on each edge.

## Rationale

**The set is what a cold open pays.** Every byte in it is downloaded, parsed and
evaluated before `<App/>` first renders. The service worker serves `/assets/*`
cache-first, so a repeat open pays parse and evaluate on the raw bytes. The
first open after a build also downloads the changed chunks, gzipped by the
engine. The shell chunk changes on nearly every frontend change.

**A shell-only budget measures where code sits, not what it costs.** It is met
by moving shell code into a chunk the shell imports statically, which still
loads before first paint. That is the `manualChunks` trick ADR 0288 rejected
for the entry. The history shows the same effect between entry and shell. The
set counts each byte once, wherever Rollup puts it, so only a real move off
the first frame lowers it.

**Raw, not gzipped.** Parse and evaluate scale with raw bytes and recur on every
cold open. The download scales with gzipped bytes and recurs once per build.
The ratio held between 3.07 and 3.12 across every build measured, so gzip would
add a second number without new information. Raw is also what Vite prints and
what the entry budget uses.

**JavaScript only.** CSS is not compiled or evaluated, so a CSS byte costs a
cold open much less than a JS byte. One number over both would let CSS growth
hide JS growth, and the reverse.

**How 1400 was chosen.** Today's set divided by 0.9 is 1428 kB, and rounded
*down* to 100 kB that is 1400. Rounding down holds a cold open at what it costs
now, rather than leaving room to grow. This change then trimmed the set under
the soft line. At 24 kB a day, soft line to budget is about six days of growth.
So the nightly goes red with nearly a week to spend.

**The entry budget still earns its place.** Entry bytes sit in front of the
startup fetches, because `startClient()` cannot run until the entry evaluates.
Shell bytes load beside those fetches. The same byte costs more in the entry,
so the entry keeps its own, tighter number inside the set's.

**Why Vite's limit is off, not raised.** Vite applies one number to every chunk
and offers no way to scope it. Any single value either holds the shell to a
lazy-chunk number or holds lazy chunks to a first-paint number. So the budget
plugin classifies each chunk and judges it by its class. No number went up.
The entry is held as before, the shell as part of a set, and lazy chunks at a
tighter 400 kB.

**Why lazy chunks only warn, and at 400.** A lazy chunk costs one surface's
open, behind a loading state or an idle prefetch, never a cold open. The
advisory exists to catch a lazy chunk that becomes a second shell or swallows a
dependency. 400 kB is 1.5 times the largest lazy chunk at adoption
(SettingsView, 263.6 kB), rounded to 100 kB. The nightly still counts the
warning.

## Consequences

- The set measured 1285.30 kB raw (418.09 kB gzipped) when this budget was
  set. Once the trims landed it measured 1233.39 kB (402.76 kB), on a tree
  that had gained the widget dock meanwhile. The shell chunk went from
  595.26 kB to 537.68 kB, and the entry from 530.22 kB to 531.79 kB. Rollup
  also split `icons` and `CategoryIcon` into chunks of their own, so the set
  is seven chunks. Every one is in `index.html`.
- **A trim leaves at least 25 kB below the soft line**, about a day of
  growth. Less, and the next day's growth turns the nightly red again, which
  is the failure this budget replaced.
- ADR 0454 adds a per-change growth check: each change reports what it adds to
  the set, and growth over 2 kB needs a stated reason.
- These on-demand surfaces moved behind dynamic imports, each idle-prefetched:
  - the brand menu's rows and the thread menu's rows;
  - both composer control menus, behind a look-alike button whose press the
    menu takes as it mounts;
  - the find bar's matcher and targets;
  - the microphone list and the widget menus' rows;
  - the menu drawer, and the confirm and prompt dialogs.
- Each opener of those surfaces waits through `whenLoaded`, which opens at
  once when the chunk is warm. `OverflowMenu` takes an `itemsChunk` for it.
  The confirm and prompt dialogs wait inside `showConfirm` and `showPrompt`
  instead: their state turns visible only once the chunk lands, and a failed
  load declines.
- A new eager import anywhere under `App.tsx` now spends first-paint budget,
  even when it lands in a separate chunk. Moving code into a chunk the shell
  imports statically no longer helps. Only a dynamic import does.
- The shell chunk has no number of its own. It may grow when the entry shrinks,
  and the reverse, as long as the set holds.
- `.build-watch/status.json` carries `chunkBudgets`, one measurement per budget,
  in place of `entryChunk`. The engine reads only `ok` and `error`.
- CSS is not budgeted. It measured 398.8 kB raw (65.7 kB gzipped) and loads
  beside the set. That gap is known and open.
- The number rests on bytes and the cache model, not on device timings.

## Alternatives considered

- **Budget the shell chunk alone.** It records where code sits, and splitting
  the shell into statically imported pieces games it, as above.
- **Budget gzipped bytes.** It tracks the once-per-build download, not the
  every-open parse, and adds nothing the stable ratio does not already give.
- **Fold CSS into the set.** Different cost per byte, and it would hide growth.
- **Raise `chunkSizeWarningLimit`.** It still could not tell first paint from
  lazy, and it would loosen the entry budget that reads the same option.
- **Keep the limit as the lazy number and let Vite apply it.** Vite applies it
  to the shell and the entry too, which is the accident this ADR removes.
- **Round 1428 up to 1500.** It would leave about 65 kB of room before the soft
  line, and so bless three more days of growth before anyone looked.
- **A committed size ratchet.** Rejected in ADR 0353: it conflicts across
  concurrent branches and hides growth below its step.
