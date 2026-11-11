import { afterEach, describe, expect, it } from 'vitest';
import type { Person } from '../../api/client';
import { addressingOf, mentionablePeople, mentionsEnabled, people, sendAddressing } from './people';

const person = (id: string, name: string): Person => ({ id, name, has_device: true });
const TEAM = [person('sam', 'Sam'), person('sam-lee', 'Sam Lee'), person('kim', 'Kim')];

afterEach(() => { people.value = { status: 'not-loaded' }; });

describe('addressingOf', () => {
  it('reads person mentions in order, each once, ignoring case', () => {
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
    people.value = { status: 'loaded', data: { people: [person('me', 'Me')], you: 'me', team: false } };
    expect(mentionsEnabled.value).toBe(false);
    expect(mentionablePeople.value).toEqual([]);
    expect(sendAddressing('@Me hi', ['me'])).toBeNull();
  });
});

describe('a send on a team gateway', () => {
  it('adds the people a compose addresses, and offers everyone but you', () => {
    people.value = { status: 'loaded', data: { people: TEAM, you: 'sam', team: true } };
    expect(sendAddressing('hello', ['kim'])).toEqual({ mentions: ['kim'], agentMentioned: false });
    expect(mentionablePeople.value.map((p) => p.id)).toEqual(['kim', 'sam-lee']);
  });
});
