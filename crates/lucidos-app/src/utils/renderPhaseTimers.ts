/** Cumulative wall-time spent in the two measurable JS phases of chat render —
 *  markdown parsing (utils/renderMarkdown, cache-miss + streaming) and linkify
 *  (utils/linkifyPaths). Feeds the `thread-render` / `thread-rerender` perf marks
 *  so an open/re-render can be split into markdown vs linkify vs the DOM/
 *  reconciliation remainder (renderMs − fetch − apply − markdown − linkify).
 *
 *  The counters are MONOTONIC and never reset at runtime — per-operation cost is
 *  a delta between two `readRenderPhaseTotals()` snapshots (baseline at the mark,
 *  read at the fire). Only the focused thread's ThreadView renders markdown/
 *  linkify, so a delta over an open/re-render window is that thread's work.
 *
 *  Strictly best-effort telemetry — see utils/perfQueue.ts and the carve-out in
 *  .claude/rules/frontend.md. Pure + unit-tested. */

let markdownParseMsTotal = 0;
let linkifyMsTotal = 0;

/** Add elapsed markdown-parse time (cache MISS / streaming only — cache hits add
 *  nothing, so the parse share isn't inflated by O(1) lookups). */
export function addMarkdownParseMs(ms: number): void {
  markdownParseMsTotal += ms;
}

/** Add elapsed linkify time (one call = one rendered block's path/app/url pass). */
export function addLinkifyMs(ms: number): void {
  linkifyMsTotal += ms;
}

export interface RenderPhaseTotals {
  markdownMs: number;
  linkifyMs: number;
}

/** Current cumulative totals. Subtract a stored baseline to get an operation's
 *  per-phase cost. */
export function readRenderPhaseTotals(): RenderPhaseTotals {
  return { markdownMs: markdownParseMsTotal, linkifyMs: linkifyMsTotal };
}

export interface PerfBaseline {
  /** performance.now() at the moment the operation (open / re-render) began. */
  start: number;
  /** markdownParseMsTotal at that moment. */
  md: number;
  /** linkifyMsTotal at that moment. */
  link: number;
}

/** Snapshot {start, md, link} to ride a perf mark. The caller stamps this when
 *  an open/re-render begins; the fire computes deltas against it. */
export function currentPerfBaseline(): PerfBaseline {
  return { start: performance.now(), md: markdownParseMsTotal, link: linkifyMsTotal };
}

/** Test-only: zero the counters so module-level state can't leak between tests. */
export function _resetRenderPhaseTimersForTesting(): void {
  markdownParseMsTotal = 0;
  linkifyMsTotal = 0;
}
