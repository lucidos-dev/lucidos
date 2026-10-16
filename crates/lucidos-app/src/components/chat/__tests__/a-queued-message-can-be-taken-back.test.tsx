// @vitest-environment jsdom
/**
 * A queued message the agent has not read offers Edit and a bin, on a Lucidos
 * Agent thread and on a Claude Code thread. Codex cannot take a message back,
 * so its queued messages offer neither. See
 * docs/plans/2026-09-29-withdraw-a-queued-claude-code-message.md.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';

import { ChatExchange } from '../ChatExchange';
import type { CodingAgent } from '../../../api/types';
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

function draw(threadIsCC: boolean, threadCodingAgent: CodingAgent = 'claude-code', isQueued = true): void {
  render(
    <ChatExchange
      exchange={queued}
      revision={0}
      streamingBuffer=""
      isLast={false}
      isQueued={isQueued}
      readMarker="sent"
      threadId="t1"
      threadIsCC={threadIsCC}
      threadCodingAgent={threadCodingAgent}
      threadIdle={false}
      threadAwaitingAnswer={false}
      threadCanceling={false}
    />,
    host,
  );
}

const edit = () => host.querySelector('.queued-message-edit');
const bin = () => host.querySelector('.queued-message-remove');

describe('a queued message', () => {
  it('offers Edit and a bin on a Claude Code thread, and reads Queued', () => {
    draw(true);
    expect(host.textContent).toContain('Queued');
    expect(host.textContent).not.toContain('Sent');
    expect(edit()).not.toBeNull();
    expect(bin()).not.toBeNull();
  });

  it('offers Edit and a bin on a Lucidos Agent thread', () => {
    draw(false);
    expect(edit()).not.toBeNull();
    expect(bin()).not.toBeNull();
  });

  it('offers neither on a Codex thread, which cannot take a message back', () => {
    draw(true, 'codex');
    expect(host.textContent).toContain('Queued');
    expect(edit()).toBeNull();
    expect(bin()).toBeNull();
  });

  it('offers neither once it is no longer queued', () => {
    draw(true, 'claude-code', false);
    expect(edit()).toBeNull();
    expect(bin()).toBeNull();
  });
});
