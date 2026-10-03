// @vitest-environment jsdom
/** A send that got no answer shows as Not sent with a Retry, never as a failed
 *  reply. A real failed reply keeps its own card. */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ChatExchange } from '../ChatExchange';
import type { StoredEvent } from '../../../store/thread-events';
import { makeExchange } from '../../../store/__tests__/fixtures';
import { unsentMessages } from '../../../store/unsentMessages';

const TS = '2026-01-01T12:00:00Z';
let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  unsentMessages.value = new Map();
});

afterEach(() => {
  act(() => { render(null, host); });
  host.remove();
  unsentMessages.value = new Map();
});

function mountFailed(userSeq: number): HTMLElement | null {
  const exchange = {
    ...makeExchange(
      { type: 'MessageReceived', text: 'hello', mode: 'human', created: TS, _eventId: 'e-1' } as StoredEvent,
      [{ seq: userSeq + 1, event: { type: 'ResponseFailed', error: 'no answer', created: TS } as StoredEvent }],
    ),
    userSeq,
  };
  act(() => {
    render(
      <ChatExchange
        exchange={exchange}
        revision={0}
        streamingBuffer=""
        isLast={true}
        threadId="tid"
        threadIsCC={false}
        threadCodingAgent="claude-code"
        threadIdle={true}
        threadAwaitingAnswer={false}
        threadCanceling={false}
      />,
      host,
    );
  });
  return host.querySelector('.exchange-error');
}

describe('the failure card', () => {
  it('reads Not sent and offers Retry for an unsent message', () => {
    unsentMessages.value = new Map([['e-1', {
      threadId: 'tid',
      body: { message: 'hello', mode: 'human', event_id: 'e-1', thread_id: 'tid' } as never,
      failedRetries: 0,
      settlement: {},
    }]]);
    const card = mountFailed(-2);
    expect(card?.querySelector('strong')?.textContent).toBe('Not sent');
    expect(card?.querySelector('button.exchange-error-retry')?.textContent?.trim()).toBe('Retry');
  });

  it('keeps the failed-reply card for a turn the engine recorded', () => {
    unsentMessages.value = new Map([['e-1', {
      threadId: 'tid',
      body: { message: 'hello', mode: 'human', event_id: 'e-1', thread_id: 'tid' } as never,
      failedRetries: 0,
      settlement: {},
    }]]);
    const card = mountFailed(10);
    expect(card?.querySelector('strong')?.textContent).toBe('The reply failed');
    expect(card?.querySelector('button')).toBeNull();
  });
});
