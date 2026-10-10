// @vitest-environment jsdom
/** The turn-limit card renders markdown. So its Settings link is a link, not
 *  `[Settings](settings)` in plain text. Its heading names the limit that
 *  fired (plan invariant 8). */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ChatExchange } from '../ChatExchange';
import type { StoredEvent } from '../../../store/thread-events';
import { makeExchange } from '../../../store/__tests__/fixtures';

const TS = '2026-01-01T12:00:00Z';
let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  act(() => { render(null, host); });
  host.remove();
});

function mountLimit(text: string): HTMLElement | null {
  const exchange = makeExchange(
    { type: 'MessageReceived', text: 'go', mode: 'human', created: TS, _eventId: 'e-1' } as StoredEvent,
    [{ seq: 1, event: { type: 'ResponseGenerated', text, created: TS } as StoredEvent }],
  );
  act(() => {
    render(
      <ChatExchange
        exchange={exchange}
        revision={0}
        streamingBuffer=""
        isLast={false}
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
  return host.querySelector('.exchange-engine-limit');
}

describe('the turn-limit card', () => {
  it('turns the tool-call cap message into a link to Settings', () => {
    const card = mountLimit(
      '[ENGINE-LIMIT] Per-turn limit of 50 tool calls reached. Send any message to continue '
      + 'from here, or raise the limit in [Settings](settings) under Models, Chat & triggers, Max tool calls.',
    );
    expect(card?.querySelector('strong')?.textContent).toBe('Tool-call limit reached');
    expect(card?.querySelector('a')?.textContent).toBe('Settings');
    expect(card?.textContent).not.toContain('](settings)');
  });

  it('does not claim a cap was reached when the turn looped instead', () => {
    const card = mountLimit(
      '[ENGINE-LIMIT] Turn ended after 300 steps without reaching the tool-call limit, which '
      + 'means it was looping without getting anywhere. Send any message to continue from here.',
    );
    expect(card?.querySelector('strong')?.textContent).toBe('Lucidos ended this turn');
  });
});
