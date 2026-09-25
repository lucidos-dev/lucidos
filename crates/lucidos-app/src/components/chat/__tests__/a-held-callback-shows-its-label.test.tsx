// @vitest-environment jsdom
/** A callback held behind an open question draws its "Held until you reply"
 *  badge. The status alone is not enough: the response panel renders only for
 *  the statuses `showStatus` lists, so a held card would draw nothing at all.
 *  See `docs/plans/2026-09-24-a-delivery-never-unparks-a-question.md`.
 */
import { afterEach, beforeEach, expect, it } from 'vitest';
import { render } from 'preact';
import { ChatExchange } from '../ChatExchange';
import type { Exchange } from '../../../store/thread-events';

const DELIVERY: Exchange = {
  userEvent: {
    type: 'UserPromptInjected',
    text: 'An event you subscribed to has arrived',
    mode: 'agent',
    created: '2026-01-01T12:00:00Z',
    _eventId: 'anchor-1',
  },
  userSeq: 4,
  steps: [],
};

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

it('shows "Held until you reply" on a delivery waiting behind a question', () => {
  render(
    <ChatExchange
      exchange={DELIVERY}
      revision={0}
      streamingBuffer=""
      isLast={true}
      threadId="tid"
      threadIsCC={false}
      threadCodingAgent="claude-code"
      threadIdle={true}
      threadAwaitingAnswer={true}
      threadCanceling={false}
    />,
    host,
  );
  const badge = host.querySelector('.exchange-status-label');
  expect(badge?.textContent).toContain('Held until you reply');
  expect(host.textContent).not.toContain('Requesting');
});
