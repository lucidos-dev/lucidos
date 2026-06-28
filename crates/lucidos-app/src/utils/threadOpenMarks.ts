/** Per-thread perf-mark baselines for the `thread-render` (open) and
 *  `thread-rerender` (send / answer) marks. Each baseline rides a perf phase
 *  snapshot ({start, md, link} — see utils/renderPhaseTimers.ts) so the fire can
 *  split the span into markdown vs linkify vs the DOM/reconciliation remainder.
 *
 *  Writers: store/actions/thread-loading.ts (open), store/actions/chat.ts +
 *  chat-claude-code.ts (re-render). Reader: components/chat/ThreadView.tsx, which
 *  takes the mark on the first render after the operation and clears it so it
 *  fires exactly once. A neutral module so telemetry plumbing doesn't reach into
 *  action internals; pure + unit-tested.
 *
 *  Strictly best-effort telemetry — see utils/perfQueue.ts and the carve-out in
 *  .claude/rules/frontend.md. */

import type { PerfBaseline } from './renderPhaseTimers';

/** What triggered a re-render, for the `thread-rerender` sample's `cause` field. */
export type RerenderCause = 'send' | 'answer';

export interface RerenderBaseline extends PerfBaseline {
  cause: RerenderCause;
}

const openMarks = new Map<string, PerfBaseline>();
const rerenderMarks = new Map<string, RerenderBaseline>();

/** Record the moment a thread OPEN begins (real event load). Overwrites any prior
 *  mark so a re-open always measures from the latest open (and a stale,
 *  never-rendered mark can't leak a multi-minute span into a later open). */
export function markThreadOpenStart(threadId: string, baseline: PerfBaseline): void {
  openMarks.set(threadId, baseline);
}

/** Read AND remove the open baseline. `undefined` when none (already taken, or
 *  never stamped) — the caller uses that to fire the render sample exactly once
 *  per open: the delete means later re-renders find nothing and don't re-fire. */
export function takeThreadOpenStart(threadId: string): PerfBaseline | undefined {
  const base = openMarks.get(threadId);
  if (base !== undefined) openMarks.delete(threadId);
  return base;
}

/** Record the moment a user-initiated RE-RENDER begins (follow-up send or answer)
 *  on the focused thread. Overwrites any pending re-render mark — only the latest
 *  user action is measured. */
export function markThreadRerenderStart(threadId: string, baseline: RerenderBaseline): void {
  rerenderMarks.set(threadId, baseline);
}

/** Read AND remove the re-render baseline (fire-once, same contract as open). */
export function takeThreadRerenderStart(threadId: string): RerenderBaseline | undefined {
  const base = rerenderMarks.get(threadId);
  if (base !== undefined) rerenderMarks.delete(threadId);
  return base;
}

/** Drop a pending re-render mark WITHOUT firing — used when the action that
 *  stamped it failed (e.g. a 409 answer with no resume), so no render is coming
 *  and the mark mustn't mis-fire a stale span on the thread's next render. */
export function clearThreadRerenderStart(threadId: string): void {
  rerenderMarks.delete(threadId);
}

/** Test-only: drop all marks so module-level state can't leak between tests. */
export function _resetThreadOpenMarksForTesting(): void {
  openMarks.clear();
  rerenderMarks.clear();
}
