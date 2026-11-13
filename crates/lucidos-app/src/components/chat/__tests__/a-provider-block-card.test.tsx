// @vitest-environment jsdom
/** A turn OpenAI blocked gets its own card. It names OpenAI and the policy.
 *  It never says "Send a message to try again". It offers a new session, on
 *  Claude Code too when a `claude` binary is usable. A provider code Lucidos
 *  has no copy for keeps the generic card and shows the raw code. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ChatExchange } from '../ChatExchange';
import { providerBlockCopy } from '../providerBlockCopy';
import type { ProviderError } from '../../../generated/thread-event-wire';
import type { StoredEvent } from '../../../store/thread-events';
import { makeExchange } from '../../../store/__tests__/fixtures';
import { unsentMessages } from '../../../store/unsentMessages';

const continueInSuccessor = vi.hoisted(() => vi.fn(async () => ({ path: 'brief', starts: 'now' })));
const getCodingAgentBinaries = vi.hoisted(() => vi.fn());
vi.mock('../../../api/client', async (importOriginal) => ({ ...(await importOriginal<object>()), continueInSuccessor, getCodingAgentBinaries }));

const TS = '2026-01-01T12:00:00Z';
const OPENAI_WORDS = 'This request was blocked by our safety systems.';
const BLOCK: ProviderError = { code: 'cyber_policy', detail: 'The request touched a restricted topic.' };
let host: HTMLDivElement;

const binaries = (claudeValid: boolean) => ({
  claude_code: { path: claudeValid ? '/bin/claude' : null, source: claudeValid ? 'path' : 'not-found', valid: claudeValid },
  codex: { path: '/bin/codex', source: 'path', valid: true },
});

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  unsentMessages.value = new Map();
  continueInSuccessor.mockClear();
  getCodingAgentBinaries.mockReset();
  getCodingAgentBinaries.mockResolvedValue(binaries(true));
});

afterEach(() => {
  act(() => { render(null, host); });
  host.remove();
});

async function mount(providerError: ProviderError | undefined, isLast = true): Promise<HTMLElement | null> {
  const exchange = makeExchange(
    { type: 'MessageReceived', text: 'hello', mode: 'human', created: TS, _eventId: 'e-1' } as StoredEvent,
    [{ seq: 2, event: { type: 'ResponseFailed', error: OPENAI_WORDS, provider_error: providerError, created: TS, _eventId: 'e-2' } as StoredEvent }],
  );
  await act(async () => {
    render(
      <ChatExchange
        exchange={exchange}
        revision={0}
        streamingBuffer=""
        isLast={isLast}
        threadId="tid"
        threadIsCC={true}
        sessionCodingAgent="codex"
        threadIdle={false}
        threadAwaitingAnswer={false}
        threadCanceling={false}
      />,
      host,
    );
  });
  return host.querySelector<HTMLElement>('.exchange-error');
}

const buttons = (card: HTMLElement | null) => [...(card?.querySelectorAll('.exchange-error-actions button') ?? [])].map((b) => b.textContent?.trim());

describe('the provider block card', () => {
  it('names OpenAI and the policy, quotes OpenAI verbatim, and never says to try again', async () => {
    const card = await mount(BLOCK);
    expect(card?.querySelector('strong')?.textContent).toBe('OpenAI blocked this turn');
    expect(card?.querySelector('.exchange-error-output')?.textContent).toBe(OPENAI_WORDS);
    expect(card?.querySelector('.exchange-error-quote')?.textContent).toBe(BLOCK.detail);
    expect(card?.querySelector('.exchange-error-reason')?.textContent).toContain('its cybersecurity policy');
    expect(card?.textContent).toContain('does not say which part triggered it');
    expect(card?.textContent).toContain('next message starts a new session');
    expect(card?.textContent).not.toContain('Send a message to try again');
  });

  it.each([
    ['misalignment_policy_violation', 'potentially unintended activity'],
    ['bio_policy', 'its biosecurity policy'],
  ])('names the %s policy in plain words', (code, words) => {
    expect(providerBlockCopy({ code })?.reason).toContain(words);
  });

  it('offers a new session and Switch to Claude Code when a claude binary is usable', async () => {
    const card = await mount(BLOCK);
    expect(buttons(card)).toEqual(['Switch to Claude Code', 'Continue in a new session']);
  });

  it('hides Switch to Claude Code when no claude binary is usable', async () => {
    getCodingAgentBinaries.mockResolvedValue(binaries(false));
    expect(buttons(await mount(BLOCK))).toEqual(['Continue in a new session']);
  });

  it('hides Switch to Claude Code when the binary probe failed', async () => {
    getCodingAgentBinaries.mockRejectedValue(new Error('offline'));
    expect(buttons(await mount(BLOCK))).toEqual(['Continue in a new session']);
  });

  it('Continue in a new session starts a successor session on the same backend', async () => {
    const card = await mount(BLOCK);
    await act(async () => { card?.querySelectorAll<HTMLButtonElement>('.exchange-error-actions button')[1].click(); });
    expect(continueInSuccessor).toHaveBeenCalledWith('tid', undefined, undefined);
  });

  it('Switch to Claude Code starts a successor session on Claude Code', async () => {
    const card = await mount(BLOCK);
    await act(async () => { card?.querySelectorAll<HTMLButtonElement>('.exchange-error-actions button')[0].click(); });
    expect(continueInSuccessor).toHaveBeenCalledWith('tid', undefined, { coding_agent: 'claude-code' });
  });

  it('offers the actions only on the latest turn', async () => {
    expect(buttons(await mount(BLOCK, false))).toEqual([]);
  });

  it('keeps the generic card for an unknown code and shows the raw code', async () => {
    const card = await mount({ code: 'something_new' });
    expect(card?.querySelector('strong')?.textContent).toBe('The reply failed');
    expect(card?.textContent).toContain(OPENAI_WORDS);
    expect(card?.textContent).toContain('Provider error code: something_new');
    expect(card?.textContent).toContain('Send a message to try again.');
    expect(card?.querySelector('button')).toBeNull();
  });
});
