// @vitest-environment jsdom
/** A coding agent that exited on its own fails the turn with a card. The card
 *  quotes the CLI, says why Lucidos did not retry, and offers Try again. A
 *  failure with no `agent_exit` keeps the old card. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ChatExchange } from '../ChatExchange';
import type { CodingAgent } from '../../../api/types';
import type { CodingAgentExit, StoredEvent } from '../../../store/thread-events';
import { makeExchange } from '../../../store/__tests__/fixtures';
import { unsentMessages } from '../../../store/unsentMessages';

const continueThread = vi.hoisted(() => vi.fn(async () => undefined));
vi.mock('../../../api/client', async (importOriginal) => ({ ...(await importOriginal<object>()), continueThread }));

const TS = '2026-01-01T12:00:00Z';
const GATEWAY_WORDS = 'Could not reach claude-gateway.example.com. Check your network connection.';
let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  unsentMessages.value = new Map();
  continueThread.mockClear();
});

afterEach(() => {
  act(() => { render(null, host); });
  host.remove();
});

function mount(opts: { agentExit?: CodingAgentExit; agent?: CodingAgent; isLast?: boolean }): HTMLElement | null {
  const exchange = makeExchange(
    { type: 'MessageReceived', text: 'hello', mode: 'human', created: TS, _eventId: 'e-1' } as StoredEvent,
    [{ seq: 2, event: { type: 'ResponseFailed', error: GATEWAY_WORDS, agent_exit: opts.agentExit, created: TS, _eventId: 'e-2' } as StoredEvent }],
  );
  act(() => {
    render(
      <ChatExchange
        exchange={exchange}
        revision={0}
        streamingBuffer=""
        isLast={opts.isLast ?? true}
        threadId="tid"
        threadIsCC={true}
        threadCodingAgent={opts.agent ?? 'claude-code'}
        threadIdle={false}
        threadAwaitingAnswer={false}
        threadCanceling={false}
      />,
      host,
    );
  });
  return host.querySelector('.exchange-error');
}

describe('the coding-agent exit card', () => {
  it('shows the heading, the CLI words verbatim, the reason and Try again for a start failure', () => {
    const card = mount({ agentExit: { stage: 'startup', code: 1 } });
    expect(card?.querySelector('strong')?.textContent).toBe('Claude Code could not start');
    expect(card?.querySelector('.exchange-error-output')?.textContent).toBe(GATEWAY_WORDS);
    expect(card?.querySelector('.exchange-error-reason')?.textContent).toBe(
      'It exited with code 1 before its session began. Lucidos did not retry, because a start failure repeats until its cause is fixed.',
    );
    expect(card?.querySelector('.exchange-error-actions button')?.textContent?.trim()).toBe('Try again');
  });

  it('names Codex for a Codex thread', () => {
    const card = mount({ agentExit: { stage: 'startup', code: 1 }, agent: 'codex' });
    expect(card?.querySelector('strong')?.textContent).toBe('Codex could not start');
  });

  it('calls a mid-turn exit a stop', () => {
    const card = mount({ agentExit: { stage: 'mid_turn', code: 1 } });
    expect(card?.querySelector('strong')?.textContent).toBe('Claude Code stopped unexpectedly');
  });

  it('Try again calls the existing continue endpoint for this thread', async () => {
    const card = mount({ agentExit: { stage: 'startup', code: 1 } });
    await act(async () => { card?.querySelector<HTMLButtonElement>('.exchange-error-actions button')?.click(); });
    expect(continueThread).toHaveBeenCalledWith('tid');
  });

  it('offers Try again only on the latest turn, since a later turn means it was already tried', () => {
    expect(mount({ agentExit: { stage: 'startup', code: 1 }, isLast: false })?.querySelector('button')).toBeNull();
  });

  it('keeps the old card for a failure with no agent_exit', () => {
    const card = mount({});
    expect(card?.querySelector('strong')?.textContent).toBe('The reply failed');
    expect(card?.querySelector('.exchange-error-output')).toBeNull();
    expect(card?.textContent).toContain('Send a message to try again.');
    expect(card?.querySelector('button')).toBeNull();
  });
});
