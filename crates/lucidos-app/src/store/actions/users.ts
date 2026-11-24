
import { computed, signal } from '@preact/signals';
import { listUsers, type UsersResponse, type UserListing, type ThreadUsers } from '../../api/client';
import { gapRefreshedVersion } from '../eventGap';
import type { Loadable } from '../types';
import { setLoadingIfFresh, toFailed } from '../types';
import { loadSlice, type SliceOf } from '../bootstrapSliceLoads';

export const users = signal<Loadable<UsersResponse>>({ status: 'not-loaded' });

export const agentPauseVersion = gapRefreshedVersion();

export const threadUsersVersion = gapRefreshedVersion();

export const shownThreadUsers = signal<{ threadId: string; users: ThreadUsers } | null>(null);

export const usersPanelOpenFor = signal<string | null>(null);

export function toggleUsersPanel(threadId: string): void {
  usersPanelOpenFor.value = usersPanelOpenFor.value === threadId ? null : threadId;
}

export const AGENT_MENTION = 'agent';

export const mentionsEnabled = computed(() => {
  const list = users.value;
  return list.status === 'loaded' && list.data.team;
});

export const mentionableUsers = computed<UserListing[]>(() => {
  const list = users.value;
  if (list.status !== 'loaded' || !list.data.team) return [];
  return list.data.users
    .filter((p) => p.id !== list.data.you)
    .sort((a, b) => a.name.localeCompare(b.name));
});

export const myUserId = computed(() => {
  const list = users.value;
  return list.status === 'loaded' ? list.data.you : null;
});

export const usersSlice: SliceOf<'users'> = {
  begin: () => setLoadingIfFresh(users),
  apply: (data) => { users.value = { status: 'loaded', data }; },
  fail: (error) => { users.value = toFailed(error); },
};

export function loadUsers(): Promise<void> {
  return loadSlice('users', usersSlice, () => listUsers());
}

export interface Addressing {
  mentions: string[];
  agentMentioned: boolean;
}

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

export function addressingOf(text: string, candidates: readonly UserListing[], senderId?: string | null): Addressing {
  const found: { at: number; id: string }[] = [];
  let rest = text;
  const take = (name: string): number[] => {
    const at: number[] = [];
    const pattern = new RegExp(`(^|[^\\w@])@${escapeRegExp(name)}(?![\\w])`, 'giu');
    rest = rest.replace(pattern, (match, lead: string, offset: number) => {
      at.push(offset + lead.length);
      return lead + ' '.repeat(match.length - lead.length);
    });
    return at;
  };
  const byLength = [...candidates].sort((a, b) => b.name.length - a.name.length);
  for (const user of byLength) {
    for (const at of take(user.name)) found.push({ at, id: user.id });
  }
  const agentMentioned = take(AGENT_MENTION).length > 0;
  const mentions: string[] = [];
  for (const { id } of found.sort((a, b) => a.at - b.at)) {
    if (id !== senderId && !mentions.includes(id)) mentions.push(id);
  }
  return { mentions, agentMentioned };
}

export function sendAddressing(text: string, alsoMention: readonly string[] = []): Addressing | null {
  if (!mentionsEnabled.value) return null;
  const list = users.value;
  if (list.status !== 'loaded') return null;
  const { mentions, agentMentioned } = addressingOf(text, list.data.users, list.data.you);
  for (const id of alsoMention) if (id !== list.data.you && !mentions.includes(id)) mentions.push(id);
  return { mentions, agentMentioned };
}
