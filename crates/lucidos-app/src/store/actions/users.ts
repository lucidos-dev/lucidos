/**
 * The users a message can address, and the *agent pause* (ADR 0649).
 *
 * The engine owns who is a user (`GET /api/v1/users`), so the
 * composer never guesses from the gateway's own list. Mentions are plain
 * prompt text: `@Sam` names a user and `@agent` the thread's agent. The
 * engine decides from them whether a message wakes the agent.
 */

import { computed, signal } from '@preact/signals';
import { listUsers, type UsersResponse, type UserListing, type ThreadUsers } from '../../api/client';
import { gapRefreshedVersion } from '../eventGap';
import type { Loadable } from '../types';
import { setLoadingIfFresh, toFailed } from '../types';

export const users = signal<Loadable<UsersResponse>>({ status: 'not-loaded' });

/** Bumped by every `AgentPaused` and `AgentResumed`, so the composer re-reads
 *  whether the agent is paused. */
export const agentPauseVersion = gapRefreshedVersion();

/** Bumped by every thread role change, so the open thread's users re-read. */
export const threadUsersVersion = gapRefreshedVersion();

/** The open thread's users as the panel's owner last read them, which the
 *  title row's icon draws from. `null` while unread or off a team gateway. */
export const shownThreadUsers = signal<{ threadId: string; users: ThreadUsers } | null>(null);

/** The thread whose users panel is open. Naming the thread, not a flag,
 *  closes the panel when another thread opens. */
export const usersPanelOpenFor = signal<string | null>(null);

export function toggleUsersPanel(threadId: string): void {
  usersPanelOpenFor.value = usersPanelOpenFor.value === threadId ? null : threadId;
}

/** The token that mentions the thread's agent. */
export const AGENT_MENTION = 'agent';

/** Two or more users: only then does a send take mentions, and only then do
 *  the users icon and the pause placeholder draw. A personal gateway looks
 *  exactly as before. */
export const mentionsEnabled = computed(() => {
  const list = users.value;
  return list.status === 'loaded' && list.data.team;
});

/** The users the composer offers, this device's user left out. */
export const mentionableUsers = computed<UserListing[]>(() => {
  const list = users.value;
  if (list.status !== 'loaded' || !list.data.team) return [];
  return list.data.users
    .filter((p) => p.id !== list.data.you)
    .sort((a, b) => a.name.localeCompare(b.name));
});

/** This device's user, or `null`. */
export const myUserId = computed(() => {
  const list = users.value;
  return list.status === 'loaded' ? list.data.you : null;
});

export async function loadUsers(): Promise<void> {
  setLoadingIfFresh(users);
  try {
    users.value = { status: 'loaded', data: await listUsers() };
  } catch (error) {
    users.value = toFailed(error);
  }
}

export interface Addressing {
  /** User ids, each once, in the order they appear. */
  mentions: string[];
  agentMentioned: boolean;
}

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** Who a message's text mentions. A mention is `@` and a name, not inside a
 *  word, and not running on into one. Longer names match first, so `@Sam Lee`
 *  never also counts as `@Sam`. Case does not matter. */
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

/** What a send carries on a team gateway: the text's mentions, plus anyone
 *  the send addresses outright (a compose to a teammate). Nothing elsewhere. */
export function sendAddressing(text: string, alsoMention: readonly string[] = []): Addressing | null {
  if (!mentionsEnabled.value) return null;
  const list = users.value;
  if (list.status !== 'loaded') return null;
  const { mentions, agentMentioned } = addressingOf(text, list.data.users, list.data.you);
  for (const id of alsoMention) if (id !== list.data.you && !mentions.includes(id)) mentions.push(id);
  return { mentions, agentMentioned };
}
