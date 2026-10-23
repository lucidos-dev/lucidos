// @vitest-environment jsdom
/** Anything waiting on an open question reads as not read yet: dimmed, with a
 *  line saying when the agent reads it. A held callback draws no response panel,
 *  so no empty agent header sits beside it. A queued message dims only while a
 *  question blocks it. See
 *  `docs/plans/2026-10-04-held-callbacks-read-as-not-read-yet.md`.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ChatExchange } from '../ChatExchange';
import { HeldMessageRow } from '../chat-exchange-parts';
import { eventRowBody } from '../EventRow';
import { HELD_CALLBACK_NOTE, HELD_MESSAGE_NOTE } from '../../../store/exchange-status';
import type { Exchange } from '../../../store/thread-events';

const PROSE_DELIVERY: Exchange = {
  userEvent: {
    type: 'PromptInjected',
    text: 'An event you subscribed to has arrived',
    mode: 'agent',
    created: '2026-01-01T12:00:00Z',
    _eventId: 'anchor-1',
  },
  userSeq: 4,
  steps: [],
};

const CHILD_RETURNED = {
  userEvent: {
    type: 'ChildThreadCompleted',
    child_thread_id: 'c1',
    child_thread_title: 'Copy file path from editor',
    status: 'success',
    summary: 'done',
    created: '2026-01-01T12:00:00Z',
    _eventId: 'child-1',
  },
  userSeq: 5,
  steps: [],
} as Exchange;

const QUEUED: Exchange = {
  userEvent: {
    type: 'MessageReceived',
    text: 'Also check the diff view',
    created: '2026-01-01T12:01:00Z',
    _eventId: 'queued-1',
  },
  userSeq: 6,
  steps: [],
};

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

// Rendering inside act flushes effects before the test ends. A bare render
// leaves Preact's after-paint timer pending past jsdom's teardown.
afterEach(() => {
  act(() => { render(null, host); });
  host.remove();
});

function draw(exchange: Exchange, awaitingAnswer: boolean, extra: { isQueued?: boolean } = {}) {
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
        threadIdle={awaitingAnswer}
        threadAwaitingAnswer={awaitingAnswer}
        threadCanceling={false}
        {...extra}
      />,
      host,
    );
  });
}

describe('a callback held behind an open question', () => {
  it('puts the note inside the child card and dims only the card', () => {
    draw(CHILD_RETURNED, true);
    const row = host.querySelector('.event-row');
    expect(row?.hasAttribute('data-held')).toBe(true);
    expect(row?.querySelector('.event-row-held-note')?.textContent).toBe(HELD_CALLBACK_NOTE);
    expect(host.querySelector('.initiator-panel')?.hasAttribute('data-held')).toBe(false);
  });

  it('draws no response panel, so no empty agent header', () => {
    draw(CHILD_RETURNED, true);
    expect(host.querySelector('.response-panel')).toBeNull();
    expect(host.textContent).not.toContain('Requesting');
  });

  it('notes and dims a prose body that has no card', () => {
    draw(PROSE_DELIVERY, true);
    const panel = host.querySelector('.initiator-panel');
    expect(panel?.hasAttribute('data-held')).toBe(true);
    expect(panel?.querySelector('.initiator-held-note')?.textContent).toBe(HELD_CALLBACK_NOTE);
    expect(host.querySelector('.response-panel')).toBeNull();
  });

  it('drops the note and returns the panel once answered', () => {
    draw(CHILD_RETURNED, false);
    expect(host.textContent).not.toContain(HELD_CALLBACK_NOTE);
    expect(host.querySelector('[data-held]')).toBeNull();
    expect(host.querySelector('.response-panel')).not.toBeNull();
  });
});

describe('a queued message', () => {
  it('dims behind an open question, keeping Queued and its buttons', () => {
    draw(QUEUED, true, { isQueued: true });
    const panel = host.querySelector('.initiator-panel');
    expect(panel?.hasAttribute('data-held')).toBe(true);
    expect(panel?.querySelector('.initiator-held-note')).toBeNull();
    expect(host.textContent).toContain('Queued');
    expect(host.querySelector('.queued-message-edit')).not.toBeNull();
    expect(host.querySelector('.queued-message-remove')).not.toBeNull();
  });

  it('stays at full strength behind a running turn', () => {
    draw(QUEUED, false, { isQueued: true });
    expect(host.querySelector('[data-held]')).toBeNull();
  });
});

describe('the coding-agent held message', () => {
  const held = { type: 'held_message', held_id: 'h1', sender: 'Lucidos Agent', text: 'hi', released: false } as const;

  it('dims with the delivery note while held', () => {
    act(() => { render(<HeldMessageRow event={held} />, host); });
    const row = host.querySelector('.event-row');
    expect(row?.hasAttribute('data-held')).toBe(true);
    expect(row?.querySelector('.event-row-held-note')?.textContent).toBe(HELD_MESSAGE_NOTE);
    expect(row?.querySelector('.event-row-state')).toBeNull();
  });

  it('reads Delivered at full strength once released', () => {
    act(() => { render(<HeldMessageRow event={{ ...held, released: true }} />, host); });
    const row = host.querySelector('.event-row');
    expect(row?.hasAttribute('data-held')).toBe(false);
    expect(row?.querySelector('.event-row-state')?.textContent).toBe('Delivered');
  });
});

it('an event row without a held note neither dims nor notes', () => {
  act(() => { render(eventRowBody({ kind: 'delivery', subject: 'x', stateLabel: 'Arrived' }), host); });
  expect(host.querySelector('[data-held]')).toBeNull();
  expect(host.querySelector('.event-row-held-note')).toBeNull();
});
