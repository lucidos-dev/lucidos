import { computed, effect, signal } from '@preact/signals';
import {
  blockedThreadIds, drawerGrouping, focusedThreadId, persistIdSet, restoreIdSet, selectedOngoingGroup,
  threadListFetched, threadMap,
} from './store';

function blockKey(id: string, lastAgentAction: string | undefined): string {
  return `${id} ${lastAgentAction ?? ''}`;
}

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
  if (!threadListFetched.value) return;
  const listingBlocked = drawerGrouping.value === 'ongoing' && selectedOngoingGroup.value === 'blocked';
  const next = seenBlockedAfter(seenBlockedState.peek(), blockKeys.value, listingBlocked ? 'all' : focusedThreadId.value);
  if (sameKeys(next, seenBlockedState.peek())) return;
  seenBlockedState.value = next;
  persistIdSet(SEEN_BLOCKED_KEY, next);
});

export const hasUnseenBlocked = computed(() => {
  const seen = seenBlockedState.value;
  for (const key of blockKeys.value.values()) if (!seen.has(key)) return true;
  return false;
});
