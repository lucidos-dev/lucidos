/**
 * Which status dot a thread wears. Pure derivation with no markup, so the
 * store and the drawer's family graph can read it without pulling the icon
 * set into the entry chunk. `ThreadStatusIcon.tsx` draws the dot.
 */

import type { ThreadMeta, ThreadStatus, ThreadState } from '../../store/thread-events';
import { changeReadyToReview } from '../../store/thread-events';
import { effectiveThreadStatus } from '../../store/store';

/** 'changes' = diff glyph (CC has pending changes). 'read-request' = static
 *  dot (its agent asked the user to read the reply, ADR 0409). 'question' =
 *  "?" badge (CC paused on AskUserQuestion). 'waiting' = pulsing dot. It shows
 *  only when the thread has no state of its own to surface: own state wins,
 *  even with active children or a live subscription. */
export type VisualStatus = ThreadStatus | 'changes' | 'read-request' | 'question';

/** `waiting` covers BOTH ways a thread can be finished-but-not-done: it is
 *  waiting on child threads it spawned, or on an *event wait* it registered.
 *  A child counts while it is mid-turn, and while it idles on its own event
 *  wait, since that child has not finished either (ADR 0254).
 *  Neither holds the thread's turn, so both land here with a backend
 *  `status` of `idle`, and both mean the same thing to the reader: something
 *  else will wake this, do not treat it as finished. They deliberately share
 *  one dot rather than splitting into two, since the distinction between them
 *  is detail the waiting indicator carries.
 *
 *  The two causes rank differently against `changes`, because the Apply gate
 *  treats them differently. A live event wait outranks it: the thread wakes on
 *  its own branch, so `availableThreadActions` withholds Apply and Discard. A
 *  running sub-thread ranks below it: the child writes its own worktree, so a
 *  parent with a proposed change reads "Changes to review" and offers Apply
 *  (ADR 0249). `is_blocking`, `is_attention_needing` and `displaySection` never
 *  see either, so a parked thread stays archivable when it has no change to
 *  resolve (ADR 0049). `VisualStatus` is the right home for the fact because
 *  it is already a derived axis: `changes` and `question` are not
 *  `ThreadStatus` values either. */
export function resolveVisualStatus(
  status: ThreadStatus,
  waitsOnSubThreads: boolean,
  changeReady: boolean,
  hasLiveEventWaits: boolean,
  readRequested = false,
): VisualStatus {
  if (status === 'failed') return 'failed';
  if (status === 'running') return 'running';
  if (status === 'waiting_for_user_answer') return 'question';
  // The user's own version switch interrupted this turn, and the engine is
  // bringing it back. It outranks `changes` for the same reason `failed` does:
  // it describes what happened to the turn, which the user needs before they
  // decide whether to review anything.
  //
  // THIS function is where that precedence lives, and the only place. The
  // backend used to state it a second time, writing `waiting` instead of the
  // verdict whenever a change was pending. That cost the verdict outright, so
  // an interrupted thread with a change came back reading Running. See
  // `docs/plans/2026-08-22-a-restart-verdict-survives-a-pending-change.md`.
  if (status === 'paused') return 'paused';
  // A live event wait outranks `changes`: the thread is not finished, so its
  // change is not final and cannot be resolved yet. Reading it as "Changes to
  // review" invited an Apply that would merge a branch still being worked on.
  if (hasLiveEventWaits) return 'waiting';
  if (changeReady) return 'changes';
  // Ranked like a change, for the same reason: Review lists the thread once
  // its turn has ended, whatever its sub-threads are doing.
  if (readRequested) return 'read-request';
  if (waitsOnSubThreads) return 'waiting';
  return 'idle';
}

/** The meta facts `visualStatusFor` reads. */
type VisualStatusFacts = Pick<
  ThreadMeta,
  'activeChildrenCount' | 'waitingChildrenCount' | 'codingAgentChangeState' | 'liveEventWaitCount' | 'readRequested'
>;

/** `resolveVisualStatus` fed from a thread's meta, for a surface holding a
 *  status snapshot of its own. `undefined` meta is a thread the client has not
 *  loaded, which resolves on the status alone. Every surface that paints a
 *  dot goes through here, so none of them can drop one of the inputs. */
export function visualStatusFor(status: ThreadStatus, meta: VisualStatusFacts | undefined): VisualStatus {
  if (!meta) return resolveVisualStatus(status, false, false, false);
  return resolveVisualStatus(
    status,
    meta.activeChildrenCount + (meta.waitingChildrenCount ?? 0) > 0,
    changeReadyToReview(meta),
    meta.liveEventWaitCount > 0,
    meta.readRequested === true,
  );
}

/** The single source of truth for a thread's status dot. The drawer row, the
 *  desktop panel header and the mobile title bar all call THIS with the same
 *  live thread from `threadMap`. So the dot cannot disagree between surfaces:
 *  same thread, same status, everywhere. Don't rebuild the
 *  `resolveVisualStatus(effectiveThreadStatus(t), …)` triple at a call site. */
export function threadVisualStatus(thread: ThreadState): VisualStatus {
  return visualStatusFor(effectiveThreadStatus(thread), thread.meta);
}
