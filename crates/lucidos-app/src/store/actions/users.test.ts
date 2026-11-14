import { afterEach, describe, expect, it } from 'vitest';
import type { UserListing } from '../../api/client';
import { addressingOf, mentionableUsers, mentionsEnabled, users, sendAddressing } from './users';

const user = (id: string, name: string): UserListing => ({ id, name, has_device: true });
const TEAM = [user('sam', 'Sam'), user('sam-lee', 'Sam Lee'), user('kim', 'Kim')];

afterEach(() => { users.value = { status: 'not-loaded' }; });

describe('addressingOf', () => {
  it('reads user mentions in order, each once, ignoring case', () => {
    expect(addressingOf('@kim and @Sam, then @KIM again', TEAM)).toEqual({ mentions: ['kim', 'sam'], agentMentioned: false });
  });

  it('matches the longer name first, so "@Sam Lee" is not also Sam', () => {
    expect(addressingOf('ask @Sam Lee', TEAM).mentions).toEqual(['sam-lee']);
  });

  it('needs the @ at a word start and the name to end there', () => {
    expect(addressingOf('mail me@kim.example or @Kimberly', TEAM).mentions).toEqual([]);
    expect(addressingOf("@Kim's draft", TEAM).mentions).toEqual(['kim']);
  });

  it('reads the agent mention', () => {
    expect(addressingOf('@agent go ahead', TEAM)).toEqual({ mentions: [], agentMentioned: true });
    expect(addressingOf('the agentic @agents', TEAM).agentMentioned).toBe(false);
  });
});

describe('a send on a personal gateway carries no addressing', () => {
  it('sends nothing and offers nobody', () => {
    users.value = { status: 'loaded', data: { users: [user('me', 'Me')], you: 'me', team: false } };
    expect(mentionsEnabled.value).toBe(false);
    expect(mentionableUsers.value).toEqual([]);
    expect(sendAddressing('@Me hi', ['me'])).toBeNull();
  });
});

describe('a send on a team gateway', () => {
  it('adds the users a compose addresses, and offers everyone but you', () => {
    users.value = { status: 'loaded', data: { users: TEAM, you: 'sam', team: true } };
    expect(sendAddressing('hello', ['kim'])).toEqual({ mentions: ['kim'], agentMentioned: false });
    expect(mentionableUsers.value.map((p) => p.id)).toEqual(['kim', 'sam-lee']);
  });
});

it('ignores self mentions after matching full names, including compose recipients', () => {
  users.value = { status: 'loaded', data: { users: TEAM, you: 'sam-lee', team: true } };
  expect(sendAddressing('@Sam Lee')).toEqual({ mentions: [], agentMentioned: false });
  expect(sendAddressing('@Sam Lee @Kim @agent', ['sam-lee'])).toEqual({ mentions: ['kim'], agentMentioned: true });
});
