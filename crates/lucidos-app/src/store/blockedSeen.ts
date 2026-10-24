import { computed, effect, signal } from '@preact/signals';
import { blockedThreadIds, drawerGrouping, focusedThreadId, selectedOngoingGroup } from './store';

/** The next seen set: the blocked threads seen since each became blocked,
 *  plus what `looking` shows now. A thread leaves the set when it stops being
 *  blocked, so blocking again counts as new. `looking` is what is on screen: every blocked thread
 *  while the Blocked group lists, else the open thread. */
export function seenBlockedAfter(
  seen: ReadonlySet<string>,
  blocked: ReadonlySet<string>,
  looking: 'all' | string | null,
): ReadonlySet<string> {
  if (looking === 'all') return blocked;
  const next = new Set<string>();
  for (const id of seen) if (blocked.has(id)) next.add(id);
  if (looking !== null && blocked.has(looking)) next.add(looking);
  return next;
}

function sameIds(a: ReadonlySet<string>, b: ReadonlySet<string>): boolean {
  if (a.size !== b.size) return false;
  for (const id of a) if (!b.has(id)) return false;
  return true;
}

const seenBlockedState = signal<ReadonlySet<string>>(new Set());

effect(() => {
  const listingBlocked = drawerGrouping.value === 'ongoing' && selectedOngoingGroup.value === 'blocked';
  const next = seenBlockedAfter(seenBlockedState.peek(), blockedThreadIds.value, listingBlocked ? 'all' : focusedThreadId.value);
  if (!sameIds(next, seenBlockedState.peek())) seenBlockedState.value = next;
});

/** A thread is blocked that the user has not seen since it became blocked.
 *  The Blocked tile breathes while this holds. In memory only, so a reload
 *  shows every blocked thread as unseen once. */
export const hasUnseenBlocked = computed(() => {
  const seen = seenBlockedState.value;
  for (const id of blockedThreadIds.value) if (!seen.has(id)) return true;
  return false;
});
