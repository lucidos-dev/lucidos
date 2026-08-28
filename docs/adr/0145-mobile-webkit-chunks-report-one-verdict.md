# 0145: The mobile-webkit e2e project keeps its chunks and reports one verdict

- **Status**: Accepted
- **Date**: 2026-08-28

## Context

`scripts/e2e-browser.sh --webkit` runs the mobile-webkit project as 36 separate
`npx playwright test` invocations: a navigation phase of 32 chunks and a
coding-agent phase of 4, three spec files each. Every invocation prints its own
totals and returns its own exit code.

So the project never reported a verdict of its own. A green run was 36 small
green runs stacked up. No line anywhere said how many tests the project has, or
whether they all passed, and nothing checked that the chunks added up. Two
sessions chased mobile-webkit failures without ever producing a single honest
exit code for it.

The obvious reading is that the chunking is the problem. It is not: the chunking
is load-bearing, and the reporting is what was missing.

## Decision

Keep the chunking. Add the verdict.

Every invocation of a project is captured, and the harness sums the summaries
into one line per project: planned tests, passed, failed, flaky, skipped,
interrupted, did not run. When the outcomes do not account for every planned
test, that is a harness failure and the run cannot report green.

## Rationale

**Removing the chunking was tried first, on this branch, and reverted.** The
premise for removing it looked solid. The chunking exists to bound WebKit memory
across a long run, and a full unfiltered pass measured flat: 342 tests in 20.4
minutes with the compressor near 5 GB and swap at zero. Two unfiltered runs
completed at exit 0.

The third did not, and it is the run that decided this.

| | |
|---|---|
| Host | external Spotlight `mds` burst, load 142 on 18 cores |
| Compressor | 5.41 GB at start, 17.97 GB mid-run |
| Engine | healthy throughout, HTTP 200 in 21 ms |
| Browser | wedged, 15 tests at the 120-second timeout, 18 preflight discards |
| Outcome | still failing when killed; the machine rebooted later |

Two things were wrong with removing the chunking, and the second is the sharp
one.

**A wedged browser is only cleared by a fresh browser.** A wedged
`com.apple.WebKit.WebContent` holds its RSS and does not exit. A chunk boundary
kills the browser and takes those children with it, every three specs. One pass
never reclaims them until the run ends, so a wedge that survives the context
preflight compounds instead of clearing.

**The host-memory guard had nothing left to fire at.**
`check_host_memory_at_boundary` runs between projects, and `--webkit` is a single
project. Removing the chunk loop therefore left the longest, heaviest project
with zero points at which the guard could stop it, for its whole duration. That
is not a tuning regression, it is the guard being structurally inert exactly
where it was written to act.

So the chunking stays, and the reporting gap is closed directly.
`summarise_playwright_log` adds up every invocation's summary and
`report_playwright_totals` prints the project's own numbers and checks them. The
banner Playwright prints per invocation is the control: every planned test lands
in exactly one outcome bucket, retries included, so the buckets must sum to the
banner. An invocation that dies before printing its summary breaks that sum
rather than passing unnoticed.

That is the actual defect behind "chunked green is not green". The complaint was
never that chunks exist. It was that nobody added them up, so a lost invocation
looked identical to a passing one.

## Consequences

- `--webkit` prints one verdict line for the project, over however many
  invocations it took. The nightly reports a number a reader can act on.
- A chunk that ends without reporting now fails the run, through `merge_rc`, so
  it can only add a failure and never mask a test one.
- The 35 in-project memory-guard boundaries stay, and so does the browser reset
  that clears a wedge. A contended host degrades the way it did before this
  branch rather than worse.
- The tally is captured with `tee`, so `PIPESTATUS` carries Playwright's exit
  code. Reading `$?` there would read `tee`'s, which is the false-green the
  repo's own "never pipe a test command" rule warns about.
- The aggregate is computed from the list reporter's text. That is a parse, and
  its guard is the planned-versus-accounted check rather than trust.
- `playwright_file_filter` now also anchors `-f <spec>`. That flag passed a bare
  basename and had the same unanchored-regex bug the chunker had, so
  `-f chat.spec.ts` also ran `app-coding-agent-spawn-from-chat.spec.ts`.

## Alternatives considered

**Run the project unfiltered, in one pass.** Implemented, measured, and
reverted, for the two reasons above. Two clean runs on an unloaded host show
what it looks like when it works. The third shows what it costs on a contended
one. Do not re-propose without a browser-level recovery mechanism in hand.

**Keep chunking and accept no verdict.** Rejected: it is the status quo that
produced two failed sessions.

**Sum the chunks in the reader's head, or in the nightly's report.** Rejected:
the harness has the numbers and is the only thing that can also check them. A
sum computed downstream cannot tell a missing invocation from a passing one.

**Add browser-level recovery, then remove the chunking.** The context preflight
already detects a wedge and answers it too weakly, discarding the CONTEXT when
the wedge is in the browser PROCESS. Relaunching the browser there would restore
the reset without splitting the run. This is the honest route to a single
invocation and it is not implemented here: it needs its own verification under
contention, which is exactly the condition that cannot be summoned on demand.
