// @vitest-environment jsdom
/**
 * The Successor card on top of the composer, offering a *successor thread* to a
 * large coding-agent thread. It shows only at a clean idle, and only where the
 * engine's verdict for the current cache state says a handoff pays.
 */
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  fetchThreadContext: vi.fn(),
  declineSuccessorOffer: vi.fn(() => Promise.resolve()),
}));
vi.mock('../../../store/actions/compose', () => ({
  startFreshDraft: vi.fn(),
}));
vi.mock('../../../store/actions/successor', () => ({
  continueInSuccessor: vi.fn(() => Promise.resolve()),
}));

import { SuccessorCard, successorFinding } from '../SuccessorCard';
import { fetchThreadContext, declineSuccessorOffer, type ThreadContext } from '../../../api/client';
import { startFreshDraft } from '../../../store/actions/compose';
import { continueInSuccessor } from '../../../store/actions/successor';
import {
  focusedThreadId, successorCardPick, successorCardThreadId, successorOffer, threadMap, toasts,
} from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import type { StoredEvent } from '../../../store/thread-events';

type MetaOverrides = NonNullable<NonNullable<Parameters<typeof makeThreadState>[1]>['meta']>;

/** An hour on, as far as a prompt cache's warmth reaches. */
const LATER = new Date(Date.now() + 3_600_000).toISOString();

function reading(card: Partial<NonNullable<ThreadContext['card']>> | null): ThreadContext {
  return {
    input_tokens: 412_000,
    can_hand_off: true,
    offer: true,
    applied: true,
    ...(card && { card: { warm: true, cold: true, fresh_ratio: 6.2, ...card } }),
  };
}

const WARM = reading({ cold_at: LATER });
const COLD = reading({ warm: true, cold: true });

const capture = { type: 'ContextCaptured', channel: 'claude_code' } as StoredEvent;

function idleThread(meta: MetaOverrides = {}, events: StoredEvent[] = [capture]) {
  const thread = makeThreadState('t1', { meta: { channel: 'claude_code', section: 'inbox', ...meta } });
  events.forEach((event, i) => thread.events.set(i + 1, event));
  return thread;
}

const now = Date.parse('2026-10-20T12:00:00Z');

describe('successorOffer', () => {
  it('offers the warm card while the cache is warm, and the cold one after cold_at', () => {
    const card = { cold_at: '2026-10-20T12:05:00Z' };
    expect(successorOffer(idleThread(), reading(card), now)).toBe('warm');
    expect(successorOffer(idleThread(), reading(card), now + 6 * 60_000)).toBe('cold');
  });

  it('reads a cache with no cold_at as already cold', () => {
    expect(successorOffer(idleThread(), COLD, now)).toBe('cold');
  });

  it('offers nothing where the verdict for the current cache state says no', () => {
    expect(successorOffer(idleThread(), reading({ warm: false, cold: true, cold_at: LATER }), now)).toBeNull();
    expect(successorOffer(idleThread(), reading({ warm: true, cold: false }), now)).toBeNull();
    expect(successorOffer(idleThread(), reading({ warm: false, cold: false }), now)).toBeNull();
  });

  it('offers nothing until the context is read, or when the engine gave no card', () => {
    expect(successorOffer(idleThread(), null, now)).toBeNull();
    expect(successorOffer(idleThread(), reading(null), now)).toBeNull();
  });

  describe('blocks on each unclean state', () => {
    const blocked: [string, MetaOverrides][] = [
      ['a running turn', { status: 'running' }],
      ['an open question card', { status: 'waiting_for_user_answer' }],
      ['a failed session', { status: 'failed' }],
      ['an event wait', { liveEventWaitCount: 1 }],
      ['a pending change', { codingAgentChangeState: { kind: 'proposed', requires_restart: false } }],
      ['withheld work', { codingAgentChangeState: { kind: 'unproposed', reason: 'hardening_missing' } }],
      ['a thread that already handed off', { successor: { thread_id: 's1', title: 'Next' } }],
      ['an archived thread', { section: 'archived' }],
      ['a chat thread', { channel: 'chat' }],
    ];
    for (const [name, meta] of blocked) {
      it(name, () => {
        expect(successorOffer(idleThread(meta), COLD, now)).toBeNull();
      });
    }

    it('a message sent and not yet confirmed', () => {
      const thread = idleThread();
      thread.pendingUserMessages = [{ text: 'next', eventId: 'e1', created: '2026-01-01T00:00:01Z' }];
      expect(successorOffer(thread, COLD, now)).toBeNull();
    });

    it('a queued message the agent has not read', () => {
      const message = (id: string) => ({ type: 'MessageReceived', text: 'x', channel: 'claude_code', _eventId: id }) as StoredEvent;
      const read = (id: string) => ({ type: 'CodingAgentInputRead', input_event_id: id }) as StoredEvent;
      const thread = idleThread({}, [message('m1'), read('m1'), capture, message('m2')]);
      expect(successorOffer(thread, COLD, now)).toBeNull();
    });
  });
});

describe('successorFinding', () => {
  it('names the size and the ratio to a fresh thread', () => {
    expect(successorFinding(reading({ fresh_ratio: 6.2 }))).toBe(
      'This thread holds 412k tokens, and every reply rereads all of it (about 6x a fresh thread).',
    );
    expect(successorFinding(reading({ fresh_ratio: 1.44 }))).toContain('about 1.4x a fresh thread');
    expect(successorFinding({ ...WARM, input_tokens: null })).toContain('This thread is large,');
  });
});

describe('SuccessorCard', () => {
  beforeEach(() => {
    vi.mocked(fetchThreadContext).mockResolvedValue(WARM);
  });

  const hosts: HTMLElement[] = [];

  afterEach(() => {
    // Clearing the body leaves a Preact tree subscribed, so unmount each one.
    for (const host of hosts.splice(0)) render(null, host);
    document.body.innerHTML = '';
    threadMap.value = new Map();
    focusedThreadId.value = null;
    successorCardPick.value = null;
    toasts.value = [];
    vi.clearAllMocks();
  });

  async function mount(meta: MetaOverrides = {}): Promise<HTMLElement> {
    threadMap.value = new Map([['t1', idleThread(meta)]]);
    focusedThreadId.value = 't1';
    const host = document.createElement('div');
    document.body.appendChild(host);
    hosts.push(host);
    await act(async () => { render(<SuccessorCard />, host); });
    await act(async () => { await Promise.resolve(); });
    return host;
  }

  const option = (host: HTMLElement, label: string) =>
    Array.from(host.querySelectorAll<HTMLButtonElement>('.successor-card .question-option'))
      .find((b) => b.querySelector('.question-option-label')?.textContent === label)!;

  it('asks the question with three options, each with its explanation', async () => {
    const host = await mount();
    expect(host.querySelector('.question-text')?.textContent).toContain('This thread holds 412k tokens');
    expect(host.querySelector('.question-text p:last-child')?.textContent).toBe('Carry on in a new thread?');
    const labels = Array.from(host.querySelectorAll('.successor-card .question-option-label')).map((e) => e.textContent);
    expect(labels).toEqual(['Continue with context', 'Start fresh', 'Stay here']);
    expect(option(host, 'Start fresh').textContent).toContain('no carry-over');
    expect(successorCardThreadId.value).toBe('t1');
  });

  it('continues with no implementer when nothing is picked', async () => {
    const host = await mount();
    await act(async () => { option(host, 'Continue with context').click(); });
    expect(continueInSuccessor).toHaveBeenCalledWith('t1', undefined);
    expect(startFreshDraft).not.toHaveBeenCalled();
  });

  it('continues on the picked model and effort, and names the pick', async () => {
    successorCardPick.value = { threadId: 't1', model: 'claude-opus-5-6', effort: 'high' };
    const host = await mount();
    expect(option(host, 'Continue with context').textContent).toContain('It runs on');
    await act(async () => { option(host, 'Continue with context').click(); });
    expect(continueInSuccessor).toHaveBeenCalledWith('t1', { coding_agent_model: 'claude-opus-5-6', reasoning_effort: 'high' });
  });

  it('shows a spinner on Continue while the engine answers, and locks the card', async () => {
    let finish!: () => void;
    vi.mocked(continueInSuccessor).mockReturnValueOnce(new Promise<void>((resolve) => { finish = resolve; }));
    const host = await mount();
    await act(async () => { option(host, 'Continue with context').click(); });
    const picked = option(host, 'Continue with context');
    expect(picked.querySelector('.mini-spinner')).not.toBeNull();
    expect(picked.textContent).toContain('Writing the handoff…');
    expect(option(host, 'Start fresh').disabled).toBe(true);
    await act(async () => { finish(); await Promise.resolve(); });
    expect(option(host, 'Start fresh').disabled).toBe(false);
  });

  it('says Starting the new thread on a cold cache', async () => {
    vi.mocked(fetchThreadContext).mockResolvedValue(COLD);
    vi.mocked(continueInSuccessor).mockReturnValueOnce(new Promise<void>(() => {}));
    const host = await mount();
    await act(async () => { option(host, 'Continue with context').click(); });
    expect(option(host, 'Continue with context').textContent).toContain('Starting the new thread…');
  });

  it('starts fresh through the draft action and calls no engine route', async () => {
    const host = await mount();
    await act(async () => { option(host, 'Start fresh').click(); });
    expect(startFreshDraft).toHaveBeenCalledWith(threadMap.value.get('t1')!.meta);
    expect(continueInSuccessor).not.toHaveBeenCalled();
    expect(declineSuccessorOffer).not.toHaveBeenCalled();
  });

  it('records Stay here with the engine', async () => {
    const host = await mount();
    await act(async () => { option(host, 'Stay here').click(); });
    expect(declineSuccessorOffer).toHaveBeenCalledWith('t1');
  });

  it('tells the user when Continue is refused, and keeps the card', async () => {
    vi.mocked(continueInSuccessor).mockRejectedValueOnce(new Error('uncommitted work'));
    const host = await mount();
    await act(async () => { option(host, 'Continue with context').click(); });
    expect(toasts.value.map((t) => t.message).join(' ')).toContain('Could not continue in a new thread');
    expect(option(host, 'Continue with context').disabled).toBe(false);
  });

  it('hides once the cache goes cold if only the warm verdict holds, with no re-read', async () => {
    vi.mocked(fetchThreadContext).mockResolvedValue(
      reading({ warm: true, cold: false, cold_at: new Date(Date.now() + 120).toISOString() }),
    );
    const host = await mount();
    expect(option(host, 'Stay here')).toBeDefined();
    await act(async () => { await new Promise((r) => setTimeout(r, 250)); });
    expect(successorCardThreadId.value).toBeNull();
    expect(fetchThreadContext).toHaveBeenCalledTimes(1);
  });

  it('keeps the pick when a warm card turns cold and stays open, and drops it when the card closes', async () => {
    vi.mocked(fetchThreadContext).mockResolvedValue(
      reading({ warm: true, cold: true, cold_at: new Date(Date.now() + 120).toISOString() }),
    );
    const pick = { threadId: 't1', model: 'claude-opus-5-6', effort: 'high' };
    successorCardPick.value = pick;
    const host = await mount();
    expect(host.querySelector('.successor-card')?.getAttribute('data-offer')).toBe('warm');
    await act(async () => { await new Promise((r) => setTimeout(r, 250)); });
    expect(host.querySelector('.successor-card')?.getAttribute('data-offer')).toBe('cold');
    expect(successorCardPick.value).toEqual(pick);
    await act(async () => { focusedThreadId.value = null; });
    expect(successorCardPick.value).toBeNull();
  });

  it('reads no context while a turn runs, and draws nothing', async () => {
    const host = await mount({ status: 'running' });
    expect(fetchThreadContext).not.toHaveBeenCalled();
    expect(host.querySelector('.successor-card')).toBeNull();
    expect(successorCardThreadId.value).toBeNull();
  });

  it('reads each thread once, and shows its reading again on a switch back', async () => {
    const host = await mount();
    threadMap.value = new Map([...threadMap.value, ['t2', makeThreadState('t2', { meta: { channel: 'claude_code', section: 'inbox' } })]]);
    await act(async () => { focusedThreadId.value = 't2'; });
    await act(async () => { await Promise.resolve(); });
    await act(async () => { focusedThreadId.value = 't1'; });
    expect(option(host, 'Continue with context')).toBeDefined();
    expect(fetchThreadContext).toHaveBeenCalledTimes(2);
  });

  it('stays open through captures that leave the size alone, and re-reads on a measured turn capture', async () => {
    const host = await mount();
    const grow = async (event: object) => {
      const thread = threadMap.value.get('t1')!;
      const next = idleThread({}, [...thread.events.values(), { ...capture, ...event } as StoredEvent]);
      await act(async () => { threadMap.value = new Map([['t1', next]]); });
      await act(async () => { await Promise.resolve(); });
    };
    const usage = { input_tokens: 412_000 };
    await grow({ usage, purpose: 'summary_compaction', producer: 'auxiliary' });
    await grow({ usage, parent_tool_use_id: 'toolu_1' });
    await grow({ purpose: 'turn' });
    expect(option(host, 'Stay here')).toBeDefined();
    expect(fetchThreadContext).toHaveBeenCalledTimes(1);
    await grow({ usage, purpose: 'turn' });
    expect(fetchThreadContext).toHaveBeenCalledTimes(2);
  });

  it('draws nothing when the context read fails', async () => {
    vi.mocked(fetchThreadContext).mockRejectedValue(new Error('engine down'));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const host = await mount();
    expect(host.querySelector('.successor-card')).toBeNull();
    warn.mockRestore();
  });
});
