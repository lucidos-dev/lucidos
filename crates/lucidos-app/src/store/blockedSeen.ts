import { computed, effect, signal } from '@preact/signals';
import {
  blockedThreadIds, drawerGrouping, focusedThreadId, persistIdSet, restoreIdSet, selectedOngoingGroup,
  threadListFetched, threadMap,
} from './store';

/** One blocked stretch of one thread: its id plus its last agent action.
 *  Every block begins with one (a question, a permission request, a failure),
 *  and a user's follow-up does not move it. */
function blockKey(id: string, lastAgentAction: string | undefined): string {
  return `${id} ${lastAgentAction ?? ''}`;
}

/** The next seen set: the blocks seen since each began, plus what `looking`
 *  shows now. `blocked` maps each blocked thread to its block key. A key leaves
 *  the set when its block ends, so blocking again counts as new. `looking` is
 *  what is on screen: every blocked thread while the Blocked group lists, else
 *  the open thread. */
export function seenBlockedAfter(
  seen: ReadonlySet<string>,
  blocked: ReadonlyMap<string, string>,
  looking: 'all' | string | null,
): ReadonlySet<string> {
  const current = new Set(blocked.values());
  if (looking === 'all') return current;
  const next = new Set<string>();
  for (const key of seen) if (current.has(key)) next.add(key);
  const lookingKey = looking === null ? undefined : blocked.get(looking);
  if (lookingKey !== undefined) next.add(lookingKey);
  return next;
}

function sameKeys(a: ReadonlySet<string>, b: ReadonlySet<string>): boolean {
  if (a.size !== b.size) return false;
  for (const key of a) if (!b.has(key)) return false;
  return true;
}

export const SEEN_BLOCKED_KEY = 'lucidos-seen-blocked-threads';

const blockKeys = computed<ReadonlyMap<string, string>>(() => {
  const keys = new Map<string, string>();
  const threads = threadMap.value;
  for (const id of blockedThreadIds.value) keys.set(id, blockKey(id, threads.get(id)?.meta.lastAgentAction));
  return keys;
});

const seenBlockedState = signal<ReadonlySet<string>>(restoreIdSet(SEEN_BLOCKED_KEY));

effect(() => {
  // Before the list arrives nothing is blocked, so pruning would forget every seen block.
  if (!threadListFetched.value) return;
  const listingBlocked = drawerGrouping.value === 'ongoing' && selectedOngoingGroup.value === 'blocked';
  const next = seenBlockedAfter(seenBlockedState.peek(), blockKeys.value, listingBlocked ? 'all' : focusedThreadId.value);
  if (sameKeys(next, seenBlockedState.peek())) return;
  seenBlockedState.value = next;
  persistIdSet(SEEN_BLOCKED_KEY, next);
});

/** A thread is blocked that the user has not seen since it became blocked.
 *  The Blocked tile breathes while this holds. The seen set persists, so a
 *  reload does not start it again. */
export const hasUnseenBlocked = computed(() => {
  const seen = seenBlockedState.value;
  for (const key of blockKeys.value.values()) if (!seen.has(key)) return true;
  return false;
});
