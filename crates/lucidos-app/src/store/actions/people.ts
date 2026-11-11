/**
 * The people a message can address, and the *agent pause* (ADR 0649).
 *
 * The engine owns who is a person (`GET /api/v1/operators/people`), so the
 * composer never guesses from the gateway's own list. Mentions are plain
 * prompt text: `@Sam` names a person and `@agent` the thread's agent. The
 * engine decides from them whether a message wakes the agent.
 */

import { computed, signal } from '@preact/signals';
import { listPeople, type PeopleResponse, type Person } from '../../api/client';
import { gapRefreshedVersion } from '../eventGap';
import type { Loadable } from '../types';
import { setLoadingIfFresh, toFailed } from '../types';

export const people = signal<Loadable<PeopleResponse>>({ status: 'not-loaded' });

/** Bumped by every `AgentPaused` and `AgentResumed`, so an open pause strip
 *  re-reads. */
export const agentPauseVersion = gapRefreshedVersion();

/** Bumped by every thread role change, so an open People row re-reads. */
export const threadPeopleVersion = gapRefreshedVersion();

/** The token that mentions the thread's agent. */
export const AGENT_MENTION = 'agent';

/** Two or more people: only then does a send take mentions, and only then do
 *  the pause strip and the People row draw. A personal gateway looks exactly
 *  as before. */
export const mentionsEnabled = computed(() => {
  const list = people.value;
  return list.status === 'loaded' && list.data.team;
});

/** The people the composer offers, this device's person left out. */
export const mentionablePeople = computed<Person[]>(() => {
  const list = people.value;
  if (list.status !== 'loaded' || !list.data.team) return [];
  return list.data.people
    .filter((p) => p.id !== list.data.you)
    .sort((a, b) => a.name.localeCompare(b.name));
});

/** This device's person, or `null`. */
export const myPersonId = computed(() => {
  const list = people.value;
  return list.status === 'loaded' ? list.data.you : null;
});

export async function loadPeople(): Promise<void> {
  setLoadingIfFresh(people);
  try {
    people.value = { status: 'loaded', data: await listPeople() };
  } catch (error) {
    people.value = toFailed(error);
  }
}

export interface Addressing {
  /** Person ids, each once, in the order they appear. */
  mentions: string[];
  agentMentioned: boolean;
}

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** Who a message's text mentions. A mention is `@` and a name, not inside a
 *  word, and not running on into one. Longer names match first, so `@Sam Lee`
 *  never also counts as `@Sam`. Case does not matter. */
export function addressingOf(text: string, candidates: readonly Person[]): Addressing {
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
  for (const person of byLength) {
    for (const at of take(person.name)) found.push({ at, id: person.id });
  }
  const agentMentioned = take(AGENT_MENTION).length > 0;
  const mentions: string[] = [];
  for (const { id } of found.sort((a, b) => a.at - b.at)) {
    if (!mentions.includes(id)) mentions.push(id);
  }
  return { mentions, agentMentioned };
}

/** What a send carries on a team gateway: the text's mentions, plus anyone
 *  the send addresses outright (a compose to a teammate). Nothing elsewhere. */
export function sendAddressing(text: string, alsoMention: readonly string[] = []): Addressing | null {
  if (!mentionsEnabled.value) return null;
  const list = people.value;
  if (list.status !== 'loaded') return null;
  const { mentions, agentMentioned } = addressingOf(text, list.data.people);
  for (const id of alsoMention) if (!mentions.includes(id)) mentions.push(id);
  return { mentions, agentMentioned };
}
