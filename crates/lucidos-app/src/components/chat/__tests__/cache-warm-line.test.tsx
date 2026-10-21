// @vitest-environment jsdom
/**
 * The *cache-warm line*: closes the feed of an idle thread, or one parked on a
 * question, while its prompt cache is still warm. The engine answers how long is left. The line counts down locally
 * and re-reads when the thread's status or events change.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type { CacheKeepAliveStatus } from '../../../generated/thread-event-wire';
import { answeringThreadIds } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import type { ThreadState } from '../../../store/thread-events';
import { applySummaryVersion } from '../../../store/thread-events/thread-meta';
import { currentCacheWarmLine } from '../../../store/actions/preferences';
import { cacheWarmLineEligible, cacheWarmText, CacheWarmLine } from '../CacheKeepAlive';

const fetchCacheWarmth = vi.fn<() => Promise<{ remaining_secs: number | null }>>();

vi.mock('../../../api/client', () => ({
  fetchCacheWarmth: (...args: unknown[]) => (fetchCacheWarmth as (...a: unknown[]) => unknown)(...args),
}));

const NOW = Date.parse('2026-10-19T10:00:00Z');

function threadWith(
  threadStatus: 'idle' | 'running' | 'waiting_for_user_answer' | 'failed',
  keepAlive: CacheKeepAliveStatus | null = null,
): ThreadState {
  return makeThreadState('t1', { meta: { status: threadStatus, cacheKeepAliveStatus: keepAlive } });
}

const root = () => document.querySelector('[data-role="cache-warm"]');

/** Render the line as `ThreadView` does, then let the engine's answer land. */
async function show(thread: ThreadState, host: HTMLElement): Promise<void> {
  await act(async () => {
    render(
      <CacheWarmLine threadId={thread.meta.id} eligible={cacheWarmLineEligible(thread)} eventCount={thread.events.size} />,
      host,
    );
  });
  await act(async () => {
    await Promise.resolve();
  });
}

async function mount(thread: ThreadState): Promise<HTMLElement> {
  const host = document.createElement('div');
  document.body.appendChild(host);
  await show(thread, host);
  return host;
}

beforeEach(() => {
  vi.useFakeTimers({ now: NOW, toFake: ['Date', 'setInterval', 'clearInterval'] });
  fetchCacheWarmth.mockReset();
});

afterEach(() => {
  document.body.innerHTML = '';
  vi.useRealTimers();
});

describe('the cache-warm line', () => {
  it('says how long an idle thread keeps its warm cache', async () => {
    fetchCacheWarmth.mockResolvedValue({ remaining_secs: 240 });
    const host = await mount(threadWith('idle'));
    expect(fetchCacheWarmth).toHaveBeenCalledWith('t1');
    expect(host.textContent).toBe('Cache warm · 4m left');
  });

  it('counts down on a local clock without asking again', async () => {
    fetchCacheWarmth.mockResolvedValue({ remaining_secs: 90 });
    const host = await mount(threadWith('idle'));
    expect(host.textContent).toBe('Cache warm · 2m left');
    await act(async () => {
      vi.advanceTimersByTime(40_000);
    });
    expect(host.textContent).toBe('Cache warm · 50s left');
    expect(fetchCacheWarmth).toHaveBeenCalledTimes(1);
  });

  it('goes when the countdown reaches zero', async () => {
    fetchCacheWarmth.mockResolvedValue({ remaining_secs: 5 });
    const host = await mount(threadWith('idle'));
    expect(host.textContent).toBe('Cache warm · 5s left');
    await act(async () => {
      vi.advanceTimersByTime(5_000);
    });
    expect(root()).toBeNull();
  });

  it('stops its clock once the countdown has run out', async () => {
    fetchCacheWarmth.mockResolvedValue({ remaining_secs: 2 });
    await mount(threadWith('idle'));
    await act(async () => {
      vi.advanceTimersByTime(2_000);
    });
    expect(vi.getTimerCount()).toBe(0);
  });

  it('draws nothing when the cache is cold', async () => {
    fetchCacheWarmth.mockResolvedValue({ remaining_secs: null });
    await mount(threadWith('idle'));
    expect(root()).toBeNull();
  });

  it('says how long a thread parked on a question keeps its warm cache', async () => {
    fetchCacheWarmth.mockResolvedValue({ remaining_secs: 240 });
    const host = await mount(threadWith('waiting_for_user_answer'));
    expect(host.textContent).toBe('Cache warm · 4m left');
  });

  it('draws nothing while an answer to the question is in flight', async () => {
    fetchCacheWarmth.mockResolvedValue({ remaining_secs: 240 });
    answeringThreadIds.value = new Set(['t1']);
    try {
      await mount(threadWith('waiting_for_user_answer'));
    } finally {
      answeringThreadIds.value = new Set();
    }
    expect(root()).toBeNull();
    expect(fetchCacheWarmth).not.toHaveBeenCalled();
  });

  it.each(['running', 'failed'] as const)('draws nothing while the thread is %s', async (status) => {
    fetchCacheWarmth.mockResolvedValue({ remaining_secs: 240 });
    await mount(threadWith(status));
    expect(root()).toBeNull();
    expect(fetchCacheWarmth).not.toHaveBeenCalled();
  });

  it('gives way to the keep-alive divider', async () => {
    fetchCacheWarmth.mockResolvedValue({ remaining_secs: 240 });
    await mount(threadWith('idle', { state: 'not_kept', reason: 'no_wake_children' }));
    await mount(threadWith('idle', {
      state: 'stopped', at: '2026-10-19T09:30:00Z', beats: 25, cost_usd: 0.64, context_tokens: 1000,
    }));
    expect(root()).toBeNull();
  });

  it('draws nothing when the read fails, and says so in the console', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    fetchCacheWarmth.mockRejectedValue(new Error('engine down'));
    await mount(threadWith('idle'));
    expect(root()).toBeNull();
    expect(warn).toHaveBeenCalledWith(expect.stringContaining('thread "t1"'), expect.any(Error));
    warn.mockRestore();
  });

  it('keeps the figure on screen while an event re-reads it', async () => {
    let answerSecond: (v: { remaining_secs: number | null }) => void = () => {};
    fetchCacheWarmth
      .mockResolvedValueOnce({ remaining_secs: 240 })
      .mockImplementationOnce(() => new Promise((resolve) => { answerSecond = resolve; }));
    const host = document.createElement('div');
    document.body.appendChild(host);
    await show(threadWith('idle'), host);
    const grown = threadWith('idle');
    grown.events.set(1, { sequence: 1 } as never);
    await show(grown, host);
    expect(host.textContent).toBe('Cache warm · 4m left');
    await act(async () => {
      answerSecond({ remaining_secs: 300 });
      await Promise.resolve();
    });
    expect(host.textContent).toBe('Cache warm · 5m left');
  });

  it('reads again when a new turn has landed, and when the thread goes idle', async () => {
    fetchCacheWarmth.mockResolvedValueOnce({ remaining_secs: 30 }).mockResolvedValueOnce({ remaining_secs: 300 });
    const host = document.createElement('div');
    document.body.appendChild(host);
    const idle = threadWith('idle');
    await show(idle, host);
    expect(host.textContent).toBe('Cache warm · 30s left');

    await show(threadWith('running'), host);
    expect(root()).toBeNull();

    await show(threadWith('idle'), host);
    expect(fetchCacheWarmth).toHaveBeenCalledTimes(2);
    expect(host.textContent).toBe('Cache warm · 5m left');
  });

  it('goes when a thread updated in place starts running', async () => {
    fetchCacheWarmth.mockResolvedValue({ remaining_secs: 240 });
    const thread = threadWith('idle');
    const host = await mount(thread);
    expect(host.textContent).toBe('Cache warm · 4m left');
    // The SSE path mutates the thread rather than replacing it.
    applySummaryVersion(thread.meta, thread.meta.summaryVersion + 1, 'running');
    await show(thread, host);
    expect(root()).toBeNull();
  });

  it('keeps an old answer from overwriting a newer one', async () => {
    let answerOld: (v: { remaining_secs: number | null }) => void = () => {};
    fetchCacheWarmth
      .mockImplementationOnce(() => new Promise((resolve) => { answerOld = resolve; }))
      .mockResolvedValueOnce({ remaining_secs: 300 });
    const host = document.createElement('div');
    document.body.appendChild(host);
    await show(threadWith('idle'), host);
    await show(threadWith('running'), host);
    await show(threadWith('idle'), host);
    expect(host.textContent).toBe('Cache warm · 5m left');
    await act(async () => answerOld({ remaining_secs: 10 }));
    expect(host.textContent).toBe('Cache warm · 5m left');
  });
});

describe('the cache-warm line switch', () => {
  it('is on until the user turns it off', () => {
    expect(currentCacheWarmLine()).toBe(true);
  });

  it('draws nothing and reads nothing when the call site passes it off', async () => {
    fetchCacheWarmth.mockResolvedValue({ remaining_secs: 240 });
    const host = document.createElement('div');
    document.body.appendChild(host);
    await act(async () => {
      render(<CacheWarmLine threadId="t1" eligible={false} eventCount={0} />, host);
    });
    expect(root()).toBeNull();
    expect(fetchCacheWarmth).not.toHaveBeenCalled();
  });
});

describe('cacheWarmText', () => {
  it('counts whole minutes up, and seconds under a minute', () => {
    expect(cacheWarmText(240)).toBe('4m left');
    expect(cacheWarmText(61)).toBe('2m left');
    expect(cacheWarmText(60)).toBe('1m left');
    expect(cacheWarmText(59)).toBe('59s left');
  });
});
