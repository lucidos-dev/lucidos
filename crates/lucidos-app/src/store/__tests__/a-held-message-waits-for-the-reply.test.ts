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
  agentMessageSender,
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

const HELD_TEXT = 'Also check the Best Buy rows';
const HELD: ThreadEvent = { type: 'MessageHeld', text: HELD_TEXT, origin: PARENT };

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

  it('leaves the transcript once its delivered copy arrives, which says it was held', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, { type: 'MessageReceived', text: 'start' }),
      ev(2, QUESTION),
      ev(3, HELD, 'held-1'),
      ev(4, { type: 'UserQuestionAnswered', tool_use_id: 'tu1', answer: { kind: 'Selected', option_id: 'a' } }),
      ev(5, { type: 'HeldMessageReleased', held_message_id: 'held-1' }),
      ev(6, { type: 'MessageReceived', text: HELD_TEXT, mode: 'agent', origin: PARENT }, 'delivered-1'),
    ]));
    const divider = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;
    const delivered = exchanges.find(e => e.userEvent._eventId === 'delivered-1')!;

    expect(exchangeResponseEvents(divider).some(e => e.type === 'held_message')).toBe(false);
    expect(delivered.releasedFromHold).toBe(true);
  });

  it('stays visible when released but never delivered, so the loss is loud', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, { type: 'MessageReceived', text: 'start' }),
      ev(2, QUESTION),
      ev(3, HELD, 'held-1'),
      ev(4, { type: 'UserQuestionAnswered', tool_use_id: 'tu1', answer: { kind: 'Selected', option_id: 'a' } }),
      ev(5, { type: 'HeldMessageReleased', held_message_id: 'held-1' }),
      ev(6, { type: 'MessageReceived', text: 'something else', mode: 'agent', origin: PARENT }, 'other-1'),
    ]));
    const divider = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;
    const other = exchanges.find(e => e.userEvent._eventId === 'other-1')!;

    expect(exchangeResponseEvents(divider).find(e => e.type === 'held_message'))
      .toMatchObject({ held_id: 'held-1', released: true });
    expect(other.releasedFromHold).toBeUndefined();
  });

  it('is never paired with a later lookalike after its own delivery was lost', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, { type: 'MessageReceived', text: 'start' }),
      ev(2, QUESTION),
      ev(3, HELD, 'held-1'),
      ev(4, { type: 'UserQuestionAnswered', tool_use_id: 'tu1', answer: { kind: 'Selected', option_id: 'a' } }),
      ev(5, { type: 'HeldMessageReleased', held_message_id: 'held-1' }),
      ev(6, { type: 'MessageReceived', text: 'unrelated', mode: 'agent', origin: PARENT }, 'other-1'),
      ev(7, { type: 'MessageReceived', text: HELD_TEXT, mode: 'agent', origin: PARENT }, 'resent-1'),
    ]));
    const divider = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;
    const resent = exchanges.find(e => e.userEvent._eventId === 'resent-1')!;

    expect(exchangeResponseEvents(divider).some(e => e.type === 'held_message')).toBe(true);
    expect(resent.releasedFromHold).toBeUndefined();
  });

  it('is not paired with the same words from another sender', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, { type: 'MessageReceived', text: 'start' }),
      ev(2, QUESTION),
      ev(3, HELD, 'held-1'),
      ev(4, { type: 'UserQuestionAnswered', tool_use_id: 'tu1', answer: { kind: 'Selected', option_id: 'a' } }),
      ev(5, { type: 'HeldMessageReleased', held_message_id: 'held-1' }),
      ev(6, {
        type: 'MessageReceived',
        text: HELD_TEXT,
        mode: 'agent',
        origin: { kind: 'thread_link', thread_id: 'other-parent', title: 'Continue the Loop' },
      }, 'other-1'),
    ]));
    const divider = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;
    const other = exchanges.find(e => e.userEvent._eventId === 'other-1')!;

    expect(exchangeResponseEvents(divider).some(e => e.type === 'held_message')).toBe(true);
    expect(other.releasedFromHold).toBeUndefined();
  });
});

describe('agentMessageSender', () => {
  it('names the sending thread or workspace, and nobody it cannot name', () => {
    expect(agentMessageSender(PARENT)).toBe('"Continue the Loop"');
    expect(agentMessageSender({ kind: 'thread_link', thread_id: 'p' })).toBe('another thread');
    expect(agentMessageSender({ kind: 'workspace', workspace: 'dev', mode: 'agent' })).toBe('workspace "dev"');
    expect(agentMessageSender(undefined)).toBeUndefined();
  });
});
