/** Per-thread perf-mark baselines for the `thread-render` (open) and
 *  `thread-rerender` (send / answer) marks. Each baseline rides a perf phase
 *  snapshot ({start, md, link} — see utils/renderPhaseTimers.ts) so the fire can
 *  split the span into markdown vs linkify vs the DOM/reconciliation remainder.
 *
 *  An open is stamped whether or not it fetches, and `OpenBaseline.warm` says
 *  which. The two halves sit apart on purpose. `focusThread` owns the WARM one,
 *  on the focus transition, and `loadThreadEvents` owns the COLD one, under its
 *  `eventsLoaded` return. Neither can cover the other: a warm open reaches no
 *  loader, and a boot that restores a thread from storage calls no `focusThread`.
 *
 *  Stamping only the cold half left thread-to-thread navigation unmeasured: a
 *  thread visited once this session never fetches again.
 *
 *  Writers: store/actions/threads.ts (warm open), thread-loading.ts (cold open),
 *  store/actions/chat.ts +
 *  chat-claude-code.ts (re-render). Reader: components/chat/ThreadView.tsx, which
 *  takes the mark on the first render after the operation and clears it so it
 *  fires exactly once. A neutral module so telemetry plumbing doesn't reach into
 *  action internals; pure + unit-tested.
 *
 *  Strictly best-effort telemetry: see utils/perfQueue.ts and the carve-out in
 *  .claude/rules/frontend.md. */

import type { PerfBaseline } from './renderPhaseTimers';

/** What triggered a re-render, for the `thread-rerender` sample's `cause` field. */
export type RerenderCause = 'send' | 'answer';

export interface RerenderBaseline extends PerfBaseline {
  cause: RerenderCause;
}

/** An open's baseline, plus whether the thread's events were ALREADY in memory.
 *
 *  The two kinds of open cost different things and the log has to
 *  tell them apart. A COLD open downloads a snapshot and folds it, so its
 *  `renderMs` is mostly network and fold. A WARM one fetches nothing: the whole
 *  span is the render, which is what back / forward between visited threads
 *  pays. Reading one number for both says nothing about either. */
export interface OpenBaseline extends PerfBaseline {
  warm: boolean;
}

const openMarks = new Map<string, OpenBaseline>();
const rerenderMarks = new Map<string, RerenderBaseline>();

/** Record the moment a thread OPEN begins. Overwrites any prior
 *  mark so a re-open always measures from the latest open (and a stale,
 *  never-rendered mark can't leak a multi-minute span into a later open). */
export function markThreadOpenStart(threadId: string, baseline: OpenBaseline): void {
  openMarks.set(threadId, baseline);
}

/** Read AND remove the open baseline. `undefined` when none (already taken, or
 *  never stamped) — the caller uses that to fire the render sample exactly once
 *  per open: the delete means later re-renders find nothing and don't re-fire. */
export function takeThreadOpenStart(threadId: string): OpenBaseline | undefined {
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
