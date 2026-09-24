/**
 * A coding agent holds an agent-sent message while the user owes it a reply
 * (ADR 0256). The transcript shows the message as held under the open
 * question, keeps the question answerable, and marks the message delivered once
 * the engine releases it.
 */
import { describe, it, expect } from 'vitest';
import {
  exchangeResponseEvents,
  exchangeStatus,
  groupIntoExchanges,
  heldMessageSender,
  type StoredEvent,
  type ThreadEvent,
} from '../thread-events';

function ev(seq: number, e: ThreadEvent, id?: string): readonly [number, StoredEvent] {
  const created = `2026-09-23T12:00:${String(seq).padStart(2, '0')}Z`;
  return [seq, { ...e, created, ...(id ? { _eventId: id } : {}) } as StoredEvent] as const;
}

const PARENT = { kind: 'thread_link', thread_id: 'parent-1', title: 'Continue the Loop' } as const;

const QUESTION: ThreadEvent = {
  type: 'UserQuestionAsked',
  tool_use_id: 'tu1',
  cc_session_id: 's',
  question: 'Run pair 2?',
  options: [{ id: 'a', label: 'Yes' }],
};

const HELD: ThreadEvent = { type: 'MessageHeld', text: 'Also check the Best Buy rows', origin: PARENT };

describe('a held message', () => {
  it('shows as held under the question, and the question stays answerable', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, { type: 'MessageReceived', text: 'start' }),
      ev(2, QUESTION),
      ev(3, HELD, 'held-1'),
    ]));
    expect(exchanges.map(e => e.userEvent.type)).toEqual(['MessageReceived', 'UserQuestionAsked']);
    const divider = exchanges[1];

    expect(divider.questionOvertaken).toBe(false);
    expect(exchangeStatus(divider, '', true, false, true)).toBe('awaiting-answer');

    const held = exchangeResponseEvents(divider).find(e => e.type === 'held_message');
    expect(held).toMatchObject({
      held_id: 'held-1',
      text: 'Also check the Best Buy rows',
      sender: '"Continue the Loop"',
      released: false,
    });
  });

  it('is marked delivered by its release, even after the transcript moved on', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, { type: 'MessageReceived', text: 'start' }),
      ev(2, QUESTION),
      ev(3, HELD, 'held-1'),
      ev(4, { type: 'UserQuestionAnswered', tool_use_id: 'tu1', answer: { kind: 'Selected', option_id: 'a' } }),
      ev(5, { type: 'ChildThreadCompleted', child_thread_id: 'c1', status: 'success', summary: 'done' } as ThreadEvent),
      ev(6, { type: 'HeldMessageReleased', held_message_id: 'held-1' }),
    ]));
    const divider = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;

    const held = exchangeResponseEvents(divider).find(e => e.type === 'held_message');
    expect(held).toMatchObject({ held_id: 'held-1', released: true });
  });
});

describe('heldMessageSender', () => {
  it('names the sending thread, a workspace, or an agent', () => {
    expect(heldMessageSender(PARENT)).toBe('"Continue the Loop"');
    expect(heldMessageSender({ kind: 'thread_link', thread_id: 'p' })).toBe('another thread');
    expect(heldMessageSender({ kind: 'workspace', workspace: 'dev', mode: 'agent' })).toBe('workspace "dev"');
    expect(heldMessageSender(undefined)).toBe('an agent');
  });
});
