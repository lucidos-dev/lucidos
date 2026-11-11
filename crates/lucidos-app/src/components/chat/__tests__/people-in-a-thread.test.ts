/** The composer's mention list, the pause row's words and the People row's
 *  marks (ADR 0649). */
import { describe, expect, it } from 'vitest';
import { mentionCandidates, mentionQueryAt } from '../mentions';
import { agentPauseState } from '../AgentPauseRow';
import { initialsOf, peopleMarks } from '../ThreadPeopleRow';

const KIM = { id: 'kim', name: 'Kim Berg', has_device: true };
const ALEX = { id: 'alex', name: 'Alex', has_device: false };

describe('mentionQueryAt', () => {
  it('finds the @ word that ends at the caret', () => {
    expect(mentionQueryAt('hi @Ki', 6)).toEqual({ query: 'Ki', start: 3 });
    expect(mentionQueryAt('@', 1)).toEqual({ query: '', start: 0 });
  });

  it('ignores an @ inside a word or one the caret has left', () => {
    expect(mentionQueryAt('me@kim', 6)).toBeNull();
    expect(mentionQueryAt('@Kim done', 9)).toBeNull();
  });
});

describe('mentionCandidates', () => {
  it('matches any word of a name, and offers the agent last', () => {
    expect(mentionCandidates('be', [KIM, ALEX], 'Claude Code').map((c) => c.token)).toEqual(['Kim Berg']);
    expect(mentionCandidates('', [KIM, ALEX], 'Claude Code').map((c) => c.token)).toEqual(['Kim Berg', 'Alex', 'agent']);
  });

  it('finds the agent by its own name too, and marks a person with no device', () => {
    expect(mentionCandidates('cla', [KIM], 'Claude Code').map((c) => c.token)).toEqual(['agent']);
    expect(mentionCandidates('al', [ALEX], 'Lucidos Agent')[0].hint).toBe('no device');
  });
});

describe('agentPauseState', () => {
  it('says who flipped the pause and how', () => {
    expect(agentPauseState(true, 'mention', 'Sam')).toBe('by Sam, mentioning only people');
    expect(agentPauseState(false, 'mention', 'you')).toBe('by you, mentioning @agent');
    expect(agentPauseState(false, 'chip', 'Sam')).toBe('by Sam');
    expect(agentPauseState(true, 'chip', null)).toBe('');
  });
});

describe('the People row', () => {
  it('draws two initials, leaving out a relay name', () => {
    expect(initialsOf('Kim Berg')).toBe('KB');
    expect(initialsOf('Sam via laptop')).toBe('SL');
  });

  it('marks the driver first, and a driver who also watches once', () => {
    const sam = { id: 'sam', name: 'Sam' };
    const marks = peopleMarks({ root_thread_id: 'r', driver: sam, watchers: [sam, { id: 'kim', name: 'Kim' }] });
    expect(marks.map((m) => `${m.person.id}:${m.role}`)).toEqual(['sam:drives', 'kim:watches']);
  });
});
