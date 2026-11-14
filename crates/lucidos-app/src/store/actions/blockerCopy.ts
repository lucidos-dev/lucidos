/** Why Archive or Delete cannot run, in the user's words (ADR 0378).
 *
 *  One table, read by the thread menu, the archive and delete refusal toasts
 *  and Archive all, so every surface says the same thing. The slugs come from
 *  the engine's *action blocker* (`thread_lifecycle::action_blocker`).
 *
 *  Pure on purpose: `threads.ts` and `threads-delete.ts` import it, and the
 *  menu's derivation imports `threads.ts`, so this module must import neither.
 */

import { ApiError } from '../../api/client';
import { OWN_BLOCKER_PRIORITY, type Blocker, type OwnBlocker } from '../../generated/thread-lifecycle';

/** A blocker that holds. `none` has no words. */
export type HeldBlocker = Exclude<Blocker, 'none'>;

/** The phrases the menu's reasons and Archive all's counts share. */
const STILL_RUNNING = 'still running';
export const WAITING_FOR_ANSWER = 'waiting for your answer';
export const CHANGE_TO_RESOLVE = 'a change to apply or discard';
const FINISHING_ITS_CHANGE = 'still waiting to finish its change';

export const BLOCKER_REASON: Record<HeldBlocker, string> = {
  home: 'The home thread always stays open.',
  running: `This thread is ${STILL_RUNNING}. Stop it first.`,
  question: `This thread is ${WAITING_FOR_ANSWER}. Answer or stop it first.`,
  pending_change: 'Apply or discard the pending change first.',
  held_proposal: `This thread is ${FINISHING_ITS_CHANGE}. Stop waiting first.`,
  descendant_running: `A sub-thread is ${STILL_RUNNING}.`,
  descendant_question: `A sub-thread is ${WAITING_FOR_ANSWER}.`,
  descendant_pending_change: `A sub-thread has ${CHANGE_TO_RESOLVE}.`,
  descendant_held_proposal: `A sub-thread is ${FINISHING_ITS_CHANGE}.`,
};

/** A blocking sub-thread's state, short enough to sit beside its title in the
 *  thread menu. */
export const SUB_THREAD_STATE: Record<OwnBlocker, string> = {
  running: 'running',
  question: 'waiting for you',
  pending_change: 'change to apply',
  held_proposal: 'finishing its change',
};

/** The note over the thread menu's list of blocking sub-threads. */
export function blockedBySubThreads(count: number): string {
  return count === 1 ? 'Blocked by a sub-thread:' : `Blocked by ${count} sub-threads:`;
}

/** The note under that list for the blocking sub-threads it does not name. */
export function moreSubThreads(count: number): string {
  return `and ${count} more`;
}

export function isHeldBlocker(value: unknown): value is HeldBlocker {
  return typeof value === 'string' && Object.prototype.hasOwnProperty.call(BLOCKER_REASON, value);
}

export function isDescendantBlocker(blocker: HeldBlocker): boolean {
  return blocker.startsWith('descendant_');
}

/** The stronger of two own blockers, by the engine's priority. */
export function strongerOwnBlocker(a: OwnBlocker | null, b: OwnBlocker | null): OwnBlocker | null {
  if (a === null) return b;
  if (b === null) return a;
  return OWN_BLOCKER_PRIORITY.indexOf(a) <= OWN_BLOCKER_PRIORITY.indexOf(b) ? a : b;
}

/** A refusal the engine explained: its blocker, and the sub-thread to show when
 *  a sub-thread blocks. */
export interface BlockedRefusal {
  blocker: HeldBlocker;
  subThreadId: string | null;
}

/** Read a cascade refusal's `blocker` slug, or `null` when `err` is not one.
 *  The first entry of `blocking` other than `targetId` is the sub-thread to
 *  show, since delete's list can name the target itself. */
export function blockedRefusal(err: unknown, targetId: string): BlockedRefusal | null {
  if (!(err instanceof ApiError) || err.httpCode !== 409) return null;
  const body = err.body as Record<string, unknown> | null | undefined;
  if (!body || typeof body !== 'object' || !isHeldBlocker(body.blocker)) return null;
  return {
    blocker: body.blocker,
    subThreadId: isDescendantBlocker(body.blocker) ? firstOtherThread(body.blocking, targetId) : null,
  };
}

function firstOtherThread(blocking: unknown, targetId: string): string | null {
  if (!Array.isArray(blocking)) return null;
  for (const member of blocking as { thread_id?: unknown }[]) {
    if (typeof member?.thread_id === 'string' && member.thread_id !== targetId) return member.thread_id;
  }
  return null;
}

/** A delete preflight member's `reason` as an own blocker. A live agent
 *  session is a turn still running. */
const MEMBER_REASON_BLOCKER: Record<string, OwnBlocker | 'home'> = {
  home_thread: 'home',
  running: 'running',
  agent_session_live: 'running',
  waiting_for_user_answer: 'question',
  pending_change: 'pending_change',
  held_proposal: 'held_proposal',
};

/** The blocker a delete preflight's `blocked_by` list amounts to, worded as
 *  the engine's own 409 would be. An unknown member reason counts as running,
 *  so a reason the client has not learned yet still blocks. */
export function blockerFromMembers(
  members: readonly { thread_id: string; reason: string }[],
  targetId: string,
): BlockedRefusal | null {
  let own: OwnBlocker | null = null;
  let descendant: OwnBlocker | null = null;
  let subThreadId: string | null = null;
  for (const member of members) {
    const mapped = MEMBER_REASON_BLOCKER[member.reason] ?? 'running';
    if (mapped === 'home') return { blocker: 'home', subThreadId: null };
    if (member.thread_id === targetId) {
      own = strongerOwnBlocker(own, mapped);
    } else if (strongerOwnBlocker(descendant, mapped) !== descendant) {
      descendant = mapped;
      subThreadId = member.thread_id;
    }
  }
  if (own !== null) return { blocker: own, subThreadId: null };
  if (descendant !== null) return { blocker: `descendant_${descendant}`, subThreadId };
  return null;
}
