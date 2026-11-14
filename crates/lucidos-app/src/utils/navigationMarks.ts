/** The intent→paint baseline for a NAVIGATION, as `threadOpenMarks` is for a
 *  thread open.
 *
 *  Two edges carry what the user calls navigation and neither had a mark: the
 *  mobile pane swap, and the content-pane view switch. A report of "lag
 *  navigating back and forth and other navigation" could only ever be half
 *  measured. Reasoning in
 *  docs/plans/2026-09-20-the-phone-can-report-what-blocks-a-navigation.md.
 *
 *  ONE pending slot, not a map. Only one navigation is ever in flight, and a
 *  second one starting means the first was abandoned rather than queued.
 *
 *  The first fire WINS, and the two fire points share the slot on purpose. On
 *  mobile `revealContentPane` swaps the pane and the view key together. That is
 *  one navigation the user made, and two samples would say it twice.
 *
 *  Best-effort telemetry, under the carve-out in .claude/rules/frontend.md. */

import { perfRecordingOn, recordPerfSample } from './perfQueue';

/** Past this age a pending mark is stale, and reporting it would LIE.
 *
 *  Every stamp has a fire point today, so this is the backstop rather than the
 *  defence. A stamp whose fire point never runs would otherwise wait, and the
 *  next navigation's effect would read the idle gap as its own span. That is
 *  worse than no sample: a plausible number nobody can trace.
 *
 *  Two seconds is far past any navigation worth calling fast, and far short of
 *  an idle stretch. A navigation slower than this is already the finding, and
 *  its own `interaction` sample still carries it. */
const MARK_STALE_MS = 2_000;

/** Which edge stamped the mark: a mobile pane swap, or a content-pane view
 *  switch.
 *
 *  Landing on a THREAD is deliberately not one of them. `thread-render`
 *  (utils/threadOpenMarks.ts) already measures that edge, and its `warm` flag is
 *  what marks thread-to-thread back and forth. */
export type NavigationKind = 'pane' | 'content';

/** Which fire point took the mark. Recorded beside `kind`, because the two can
 *  differ: a mobile content reveal is stamped as a pane swap and can be fired by
 *  either effect. */
export type NavigationFiredBy = 'pane' | 'content-view';

export interface NavigationMark {
  kind: NavigationKind;
  /** Where it is going, as a CLASS. See `navigationClass`. */
  to: string;
  /** `performance.now()` at the user's intent, before the state change. */
  start: number;
  /** The content view this stamp expects to arrive, when the intent site knows
   *  it. A fire point reporting some OTHER view refuses the mark.
   *
   *  It is what makes a no-op reveal harmless. Expanding a collapsed pane onto
   *  the view it already holds changes no key, so no fire point runs, and the
   *  mark would otherwise wait. An unrelated view change inside the staleness
   *  window would then be reported from this reveal's start time, inflated by
   *  the gap. Absent means any fire point may take it, which is right for a
   *  pane swap. */
  expectKey?: string | null;
}

let pending: NavigationMark | null = null;

/** Bumped by every stamp, so a scheduled report can tell whether the navigation
 *  it measures is still the current one.
 *
 *  Taking the mark clears `pending`, which is no longer enough on its own. Two
 *  taps inside one frame commit twice before any animation frame runs. Both
 *  reports are then scheduled, and the first would record a destination that
 *  was never painted. */
let generation = 0;

/** A content view key reduced to something safe to log.
 *
 *  A view key is built to be DISTINCT, not to be published. It can carry a file
 *  path, an app id, or a digest of a draft email's subject. The log wants to
 *  know WHICH KIND of view was slow and nothing past that, so only the leading
 *  segment is kept.
 *
 *  Pure, and the single chokepoint every caller goes through, so no payload can
 *  reach a sample by a route that forgot to strip it. */
export function navigationClass(viewKey: string | null): string {
  if (!viewKey) return 'none';
  const head = viewKey.split(':', 1)[0];
  return head || 'none';
}

/** Stamp the start of a navigation. Overwrites any unfired mark: a navigation
 *  the user replaced before it painted is not a reading anybody wants.
 *
 *  A no-op while recording is off, so the default path allocates nothing and
 *  leaves the fire points with nothing to schedule. */
export function markNavigationStart(
  kind: NavigationKind,
  to: string,
  start: number,
  expectKey?: string | null,
): void {
  if (!perfRecordingOn()) return;
  generation += 1;
  pending = { kind, to, start, expectKey };
}

/** Read AND clear the pending mark. `undefined` when there is none, which is how
 *  the fire points fire exactly once between them. */
export function takeNavigationStart(): NavigationMark | undefined {
  const mark = pending;
  pending = null;
  return mark ?? undefined;
}

/** Take the pending mark and record the span, on the frame the user sees.
 *
 *  Called from a `useLayoutEffect` that already runs on the transition being
 *  measured. The rAF then resolves just before the paint of that commit, so the
 *  span is intent to visible rather than intent to committed.
 *
 *  `arrivingKey` is the content view that landed, where the fire point knows
 *  one. It supplies the destination, which the intent site knows less precisely,
 *  and it is checked against the stamp's `expectKey`.
 *
 *  Fire-and-forget by construction: no mark means no sample, and
 *  `recordPerfSample` is itself gated and swallows its own failures. */
export function reportNavigation(firedBy: NavigationFiredBy, arrivingKey?: string | null): void {
  const mark = takeNavigationStart();
  if (!mark) return;
  // Some OTHER view arrived, so this stamp was not for it. See `expectKey`.
  if (mark.expectKey !== undefined && mark.expectKey !== (arrivingKey ?? null)) return;
  // A mark nothing fired on is not this navigation's. See `MARK_STALE_MS`.
  if (performance.now() - mark.start > MARK_STALE_MS) return;
  const to = arrivingKey !== undefined ? navigationClass(arrivingKey) : mark.to;
  // The mark just taken IS the latest stamp, so this is its generation.
  const gen = generation;
  requestAnimationFrame(() => {
    // Superseded before it ever painted. See `generation`.
    if (gen !== generation) return;
    // Checked AGAIN, inside the frame. A backgrounded page suspends animation
    // frames for as long as it is hidden, and iOS backgrounds a PWA constantly.
    // The callback then runs on resume, and the span would be the suspension.
    const ms = performance.now() - mark.start;
    if (ms > MARK_STALE_MS) return;
    recordPerfSample('navigation', { kind: mark.kind, to, firedBy, ms: Math.round(ms) });
  });
}

/** Test-only: drop the slot so one test's navigation cannot reach the next. */
export function _resetNavigationMarksForTesting(): void {
  pending = null;
  generation = 0;
}
