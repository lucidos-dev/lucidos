// @vitest-environment jsdom
/**
 * A queued message on a coding-agent thread reads "Queued" like the Lucidos
 * Agent's queue, with no bin. The agent already holds the message, so nothing
 * can take it back. See docs/plans/2026-09-24-unread-coding-agent-messages-queue.md.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';

import { ChatExchange } from '../ChatExchange';
import type { Exchange, StoredEvent } from '../../../store/thread-events';

const queued: Exchange = {
  userEvent: {
    type: 'MessageReceived',
    text: 'also check the totals',
    created: '2026-09-24T06:13:00Z',
    _eventId: 'm2',
  } as StoredEvent,
  userSeq: 2,
  steps: [],
  awaitingRead: true,
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

function draw(threadIsCC: boolean): void {
  render(
    <ChatExchange
      exchange={queued}
      revision={0}
      streamingBuffer=""
      isLast={false}
      isQueued
      readMarker="sent"
      threadId="t1"
      threadIsCC={threadIsCC}
      threadCodingAgent="claude-code"
      threadIdle={false}
      threadAwaitingAnswer={false}
      threadCanceling={false}
    />,
    host,
  );
}

describe('a queued message', () => {
  it('offers no bin on a coding-agent thread, and reads Queued alone', () => {
    draw(true);
    expect(host.textContent).toContain('Queued');
    expect(host.textContent).not.toContain('Sent');
    expect(host.querySelector('.queued-message-remove')).toBeNull();
  });

  it('keeps its bin on a Lucidos Agent thread', () => {
    draw(false);
    expect(host.querySelector('.queued-message-remove')).not.toBeNull();
  });
});
