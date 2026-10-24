// @vitest-environment jsdom
/** A keep-alive run that began while the turn waited on a question card draws
 *  its card under that card, inside the turn. The question keeps its buttons
 *  (ADR 0563). */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ChatExchange } from '../ChatExchange';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import { threadMap } from '../../../store/store';
import type { Exchange, StoredEvent } from '../../../store/thread-events';

const question = {
  type: 'UserQuestionAsked',
  tool_use_id: 'q1',
  cc_session_id: '',
  question: 'Which branch?',
  options: [{ id: 'a', label: 'main' }],
  created: '2026-10-03T14:01:00.000Z',
  _eventId: 'ask-1',
} as unknown as StoredEvent;

const warming = {
  state: 'warming', since: '2026-10-03T14:01:00.000Z', beats: 2, budget_beats: 12, beat_every_secs: 230,
  cost_usd: 0.05, context_tokens: 100_000, give_up_at: '2026-10-03T14:40:00.000Z',
} as const;

const status = { type: 'CacheKeepAliveStatusChanged', created: '2026-10-03T14:05:00.000Z', status: warming } as unknown as StoredEvent;

const parked: Exchange = { userEvent: question, userSeq: 2, steps: [{ seq: 3, event: status }] };

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  threadMap.value = new Map([['tid', makeThreadState('tid', { meta: { status: 'waiting_for_user_answer', cacheKeepAliveStatus: warming } })]]);
});

afterEach(() => {
  act(() => { render(null, host); });
  host.remove();
  threadMap.value = new Map();
});

function draw(exchange: Exchange) {
  act(() => {
    render(
      <ChatExchange
        exchange={exchange}
        revision={0}
        streamingBuffer=""
        isLast={true}
        threadId="tid"
        threadIsCC={true}
        sessionCodingAgent="claude-code"
        threadIdle={true}
        threadAwaitingAnswer={true}
        threadCanceling={false}
      />,
      host,
    );
  });
}

describe('a keep-alive card under a parked question', () => {
  it('draws the run, and the question still waits on its answer', () => {
    draw(parked);
    expect(host.querySelector('[data-role="cache-keep-alive"]')).not.toBeNull();
    expect(host.textContent).toContain('2 of 12 pings');
    expect(host.textContent).toContain('Which branch?');
  });

  it('adds no agent header of its own', () => {
    draw(parked);
    expect(host.querySelector('.response-panel')).toBeNull();
    expect(host.querySelectorAll('.initiator-panel')).toHaveLength(1);
  });

  it('draws the question card ahead of the run', () => {
    draw(parked);
    const ask = host.querySelector('.question-body');
    const run = host.querySelector('[data-role="cache-keep-alive"]');
    expect(ask).not.toBeNull();
    expect(ask!.compareDocumentPosition(run!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });
});

describe('a keep-alive card once its question is answered', () => {
  const answer = { type: 'UserQuestionAnswered', tool_use_id: 'q1', answer: { kind: 'Selected', option_id: 'a' }, created: '2026-10-03T14:06:00.000Z' } as unknown as StoredEvent;

  it('stays drawn, since the answered card draws no panel to hold it', () => {
    draw({ ...parked, steps: [...parked.steps, { seq: 4, event: answer }] });
    expect(host.querySelector('[data-role="cache-keep-alive"]')).not.toBeNull();
  });

  it('leaves the working badge to the turn, which the run never hides', () => {
    const resumed: Exchange = { ...parked, steps: [...parked.steps, { seq: 4, event: answer }] };
    act(() => {
      render(
        <ChatExchange
          exchange={resumed}
          revision={0}
          streamingBuffer=""
          isLast={true}
          threadId="tid"
          threadIsCC={true}
          sessionCodingAgent="claude-code"
          threadIdle={false}
          threadAwaitingAnswer={false}
          threadCanceling={false}
        />,
        host,
      );
    });
    expect(host.querySelector('.exchange-status-label')).not.toBeNull();
  });
});

describe('a held message under a parked question', () => {
  it('keeps the run out of the agent panel the held row opens', () => {
    const held = { type: 'MessageHeld', text: 'later', created: '2026-10-03T14:05:30.000Z', origin: { kind: 'agent' }, _eventId: 'h1' } as unknown as StoredEvent;
    draw({ ...parked, steps: [...parked.steps, { seq: 4, event: held }] });
    expect(host.querySelectorAll('[data-role="cache-keep-alive"]')).toHaveLength(1);
    expect(host.querySelector('.response-panel [data-role="cache-keep-alive"]')).toBeNull();
  });
});
