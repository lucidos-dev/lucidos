/** When the thread transcript shows its loading skeleton.
 *
 *  Two decisions live here, and they answer to the same failure. The gate side
 *  says what ThreadView paints this render. The hold side says whether a landing
 *  events snapshot must put the skeleton on screen before it is applied. See
 *  docs/plans/2026-08-14-thread-skeleton-covers-the-whole-open.md. */

import { signal } from '@preact/signals';

/** Snapshot size, in events, above which a thread open is FELT rather than
 *  instant. Below it the transcript renders straight through, as it always did.
 *
 *  A size proxy rather than a cost measurement. The events are the raw material
 *  of the render, and the markdown pass scales with them. Measured on the
 *  reported case, 370 events carrying 199k characters of markdown: the parse,
 *  apply and fold cost about 2ms on a laptop. The markdown pass alone cost 40ms,
 *  before any DOM is built. A phone is several times slower.
 *
 *  A tuning knob, deliberately set near the median thread, which held about 200
 *  events when this was written. So the big half of threads is covered, and the
 *  small half keeps the delay gate alone. Lower it if a smaller thread is still
 *  reported opening blank; raise it if a fast open shimmers for nothing. */
export const SKELETON_HOLD_EVENT_COUNT = 200;

/** The thread whose transcript must show its skeleton NOW, ahead of the usual
 *  `SPINNER_DELAY_MS` gate. Raised by the events load when it is about to apply
 *  a big snapshot, and released when that load ends. Null the rest of the time.
 *
 *  A stale id is inert rather than wrong: ThreadView only honours it for the
 *  thread it is rendering. */
export const forcedSkeletonThreadId = signal<string | null>(null);

/** Drop a forced skeleton, unless a newer load already claimed the flag. */
export function releaseForcedSkeleton(threadId: string): void {
  if (forcedSkeletonThreadId.value === threadId) forcedSkeletonThreadId.value = null;
}

export interface SkeletonHoldInput {
  /** Is this the thread the user is looking at? */
  focused: boolean;
  /** How many events the landing snapshot carries. */
  snapshotEventCount: number;
  /** Does the transcript already show turns, from a content event or from an
   *  optimistic pending message? There is then nothing for a skeleton to
   *  cover. */
  hasContent: boolean;
}

/** Must this snapshot raise the skeleton, and wait for a paint, before its rows
 *  are applied?
 *
 *  Applying rows triggers the fold and the markdown pass in one synchronous
 *  render, and nothing paints while that runs. A snapshot landing inside
 *  `SPINNER_DELAY_MS` therefore cancels a skeleton that never got a frame, and
 *  the pane stays blank for the whole render.
 *
 *  Only the focused thread qualifies. `loadAllThreads` fans the same load out
 *  over every active and saved thread, and none of those are on screen. */
export function shouldHoldForSkeleton(input: SkeletonHoldInput): boolean {
  if (!input.focused || input.hasContent) return false;
  return input.snapshotEventCount >= SKELETON_HOLD_EVENT_COUNT;
}

/** The terms ThreadView already derives per render, named so the gate below can
 *  be read without it. */
export interface ThreadLoadingState {
  hasExchanges: boolean;
  animating: boolean;
  eventsLoadFailed: boolean;
  eventsLoaded: boolean;
  disconnected: boolean;
}

/** Is the transcript mid-open, with nothing to show and nothing terminal to
 *  say? Mirrors `emptyReason`'s `loading` verdict. Also valid before the thread
 *  reaches `threadMap`, where every flag reads false. */
export function threadIsLoadingNow(state: ThreadLoadingState): boolean {
  return !state.hasExchanges
    && !state.animating
    && !state.eventsLoadFailed
    && !state.eventsLoaded
    && !state.disconnected;
}

/** Does the transcript paint its skeleton this render?
 *
 *  Only while loading, and then on either clock: the delay gate, or a load that
 *  raised it to cover the render it is about to trigger. A terminal state always
 *  wins, so a forced skeleton can never cover "No messages in this thread" or an
 *  unreachable workspace. */
export function showThreadSkeletonNow(
  state: ThreadLoadingState,
  delayElapsed: boolean,
  forced: boolean,
): boolean {
  return threadIsLoadingNow(state) && (delayElapsed || forced);
}
