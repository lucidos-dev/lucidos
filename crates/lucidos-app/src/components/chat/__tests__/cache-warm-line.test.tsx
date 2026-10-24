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
import type { CacheWarmth } from '../../../api/client/successor';
import { handleEvent, type ThreadEvent } from '../../../store/thread-events';
import { formatShortTime } from '../../../utils/formatTime';
import { cacheWarmLineEligible, CacheWarmLine } from '../CacheKeepAlive';

const fetchCacheWarmth = vi.fn<() => Promise<CacheWarmth>>();
const switchCacheKeepAlive = vi.fn<(threadId: string, state: 'off' | 'on') => Promise<void>>();

const menu = vi.hoisted(() => ({ openCacheKeepAliveSetting: vi.fn(), openCacheWarmLineSetting: vi.fn() }));

vi.mock('../../../store/actions/menu', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/menu')>()),
  ...menu,
}));

vi.mock('../../../api/client', () => ({
  fetchCacheWarmth: (...args: unknown[]) => (fetchCacheWarmth as (...a: unknown[]) => unknown)(...args),
  switchCacheKeepAlive: (...args: unknown[]) => (switchCacheKeepAlive as (...a: unknown[]) => unknown)(...args),
}));

const NOW = Date.parse('2026-10-19T10:00:00Z');

/** An idle thread with no card parked on it: nothing keeps its cache warm. */
const NOTHING_PENDING: CacheKeepAliveStatus = { state: 'not_kept', reason: 'nothing_pending' };

const warm = (remaining_secs: number, keep_alive: CacheKeepAliveStatus = NOTHING_PENDING, reread_usd: number | null = null): CacheWarmth => ({
  remaining_secs,
  keep_alive,
  reread_usd,
});
const COLD: CacheWarmth = { remaining_secs: null, keep_alive: null, reread_usd: null };

const clock = (iso: string) => formatShortTime(new Date(iso));
const KEPT_UNTIL = '2026-10-19T10:20:00Z';
const kept: CacheKeepAliveStatus = {
  state: 'warming', since: '2026-10-19T09:50:00Z', beats: 3, budget_beats: 25, beat_every_secs: 230, cost_usd: 0.412,
  context_tokens: 182_000, give_up_at: KEPT_UNTIL,
};
const stopped: CacheKeepAliveStatus = {
  state: 'stopped', at: '2026-10-19T09:58:00Z', beats: 12, cost_usd: 1.62, context_tokens: 182_000,
};
/** The pill's tail for a cache that goes cold `secs` after the read. */
const coldAt = (secs: number) => formatShortTime(new Date(NOW + secs * 1000));

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
  switchCacheKeepAlive.mockReset();
  switchCacheKeepAlive.mockResolvedValue(undefined);
  menu.openCacheKeepAliveSetting.mockReset();
  menu.openCacheWarmLineSetting.mockReset();
});

afterEach(async () => {
  // Unmount first: an open popover left on the overlay stack would swallow the
  // next test's click.
  await act(async () => {
    for (const host of Array.from(document.body.children)) render(null, host);
  });
  document.body.innerHTML = '';
  vi.useRealTimers();
});

describe('the cache-warm line', () => {
  it('says when the warm cache of an idle thread goes cold', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(240));
    const host = await mount(threadWith('idle'));
    expect(fetchCacheWarmth).toHaveBeenCalledWith('t1');
    expect(host.textContent).toBe(`Cache warm · not kept · cold at ${coldAt(240)}`);
  });

  it('shows a time, not a countdown, and reads once', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(90));
    const host = await mount(threadWith('idle'));
    await act(async () => {
      vi.advanceTimersByTime(40_000);
    });
    expect(host.textContent).toBe(`Cache warm · not kept · cold at ${coldAt(90)}`);
    expect(fetchCacheWarmth).toHaveBeenCalledTimes(1);
  });

  it('goes when the cache goes cold', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(5));
    const host = await mount(threadWith('idle'));
    expect(host.textContent).toContain('Cache warm');
    await act(async () => {
      vi.advanceTimersByTime(5_000);
    });
    expect(root()).toBeNull();
  });

  it('stops its clock once the cache has gone cold', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(2));
    await mount(threadWith('idle'));
    await act(async () => {
      vi.advanceTimersByTime(2_000);
    });
    expect(vi.getTimerCount()).toBe(0);
  });

  it('draws nothing when the cache is cold', async () => {
    fetchCacheWarmth.mockResolvedValue(COLD);
    await mount(threadWith('idle'));
    expect(root()).toBeNull();
  });

  it('says how long a thread parked on a question keeps its warm cache', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(240));
    const host = await mount(threadWith('waiting_for_user_answer'));
    expect(host.textContent).toBe(`Cache warm · not kept · cold at ${coldAt(240)}`);
  });

  it('draws nothing while an answer to the question is in flight', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(240));
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
    fetchCacheWarmth.mockResolvedValue(warm(240));
    await mount(threadWith(status));
    expect(root()).toBeNull();
    expect(fetchCacheWarmth).not.toHaveBeenCalled();
  });

  it('gives way to the keep-alive divider', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(240));
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
    let answerSecond: (v: CacheWarmth) => void = () => {};
    fetchCacheWarmth
      .mockResolvedValueOnce(warm(240))
      .mockImplementationOnce(() => new Promise((resolve) => { answerSecond = resolve; }));
    const host = document.createElement('div');
    document.body.appendChild(host);
    await show(threadWith('idle'), host);
    const grown = threadWith('idle');
    grown.events.set(1, { sequence: 1 } as never);
    await show(grown, host);
    expect(host.textContent).toBe(`Cache warm · not kept · cold at ${coldAt(240)}`);
    await act(async () => {
      answerSecond(warm(300));
      await Promise.resolve();
    });
    expect(host.textContent).toBe(`Cache warm · not kept · cold at ${coldAt(300)}`);
  });

  it('reads again when a new turn has landed, and when the thread goes idle', async () => {
    fetchCacheWarmth.mockResolvedValueOnce(warm(30)).mockResolvedValueOnce(warm(300));
    const host = document.createElement('div');
    document.body.appendChild(host);
    const idle = threadWith('idle');
    await show(idle, host);
    expect(host.textContent).toBe(`Cache warm · not kept · cold at ${coldAt(30)}`);

    await show(threadWith('running'), host);
    expect(root()).toBeNull();

    await show(threadWith('idle'), host);
    expect(fetchCacheWarmth).toHaveBeenCalledTimes(2);
    expect(host.textContent).toBe(`Cache warm · not kept · cold at ${coldAt(300)}`);
  });

  it('goes when a thread updated in place starts running', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(240));
    const thread = threadWith('idle');
    const host = await mount(thread);
    expect(host.textContent).toBe(`Cache warm · not kept · cold at ${coldAt(240)}`);
    // The SSE path mutates the thread rather than replacing it.
    applySummaryVersion(thread.meta, thread.meta.summaryVersion + 1, 'running');
    await show(thread, host);
    expect(root()).toBeNull();
  });

  it('keeps an old answer from overwriting a newer one', async () => {
    let answerOld: (v: CacheWarmth) => void = () => {};
    fetchCacheWarmth
      .mockImplementationOnce(() => new Promise((resolve) => { answerOld = resolve; }))
      .mockResolvedValueOnce(warm(300));
    const host = document.createElement('div');
    document.body.appendChild(host);
    await show(threadWith('idle'), host);
    await show(threadWith('running'), host);
    await show(threadWith('idle'), host);
    expect(host.textContent).toBe(`Cache warm · not kept · cold at ${coldAt(300)}`);
    await act(async () => answerOld(warm(10)));
    expect(host.textContent).toBe(`Cache warm · not kept · cold at ${coldAt(300)}`);
  });
});

const popover = () => document.querySelector('[role="dialog"]');

async function openExplainer(host: HTMLElement): Promise<string> {
  await act(async () => {
    host.querySelector<HTMLButtonElement>('.explainer-btn')!.click();
  });
  return popover()?.textContent ?? '';
}

async function clickLink(label: string): Promise<void> {
  const link = Array.from(popover()!.querySelectorAll<HTMLButtonElement>('button.accent-link')).find((b) => b.textContent === label)!;
  await act(async () => link.click());
}

describe('the cache-warm line while the keep-alive beats', () => {
  it('says until when the cache is kept warm, with no countdown', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(200, kept));
    const host = await mount(threadWith('waiting_for_user_answer'));
    expect(host.textContent).toBe(`Cache kept warm · until ${clock(KEPT_UNTIL)}`);
    expect(root()?.getAttribute('data-state')).toBe('warming');
    await act(async () => {
      vi.advanceTimersByTime(600_000);
    });
    expect(host.textContent).toBe(`Cache kept warm · until ${clock(KEPT_UNTIL)}`);
  });

  it('explains the cache, the run so far, and what a later reply costs', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(200, kept, 0.57));
    const text = await openExplainer(await mount(threadWith('waiting_for_user_answer')));
    expect(text).toContain("holds this thread's context in its prompt cache");
    expect(text).toContain(`3 of 25 pings · $0.41 · until ${clock(KEPT_UNTIL)}.`);
    expect(text).toContain('182k tokens once, about $0.57');
  });

  it('opens the one explainer a card and the panel open', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(200, kept));
    await openExplainer(await mount(threadWith('waiting_for_user_answer')));
    expect(popover()?.getAttribute('aria-label')).toBe('Cache kept warm');
  });

  it('links the keep-alive setting and the setting that hides the line', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(200, kept));
    await openExplainer(await mount(threadWith('waiting_for_user_answer')));
    await clickLink('Settings → Models → Cache keep-alive');
    expect(menu.openCacheKeepAliveSetting).toHaveBeenCalledTimes(1);
    expect(popover()).toBeNull();
  });

  it('goes at the give-up time, until the engine says what happened', async () => {
    const soon: CacheKeepAliveStatus = { ...kept, give_up_at: '2026-10-19T10:01:00Z' } as CacheKeepAliveStatus;
    fetchCacheWarmth.mockResolvedValue(warm(150, soon));
    const host = await mount(threadWith('waiting_for_user_answer'));
    expect(host.textContent).toContain('Cache kept warm');
    await act(async () => {
      vi.advanceTimersByTime(61_000);
    });
    expect(root()).toBeNull();
  });
});

const switchButton = (label: string) =>
  Array.from(popover()?.querySelectorAll<HTMLButtonElement>('[data-role="cache-keep-alive-switch"]') ?? []).find(
    (b) => b.textContent?.trim() === label,
  );

describe('the per-thread keep-alive switch on the cache-warm line', () => {
  const OFF_AT = '2026-10-19T09:59:00Z';
  const turnedOff: CacheKeepAliveStatus = { state: 'turned_off', at: OFF_AT, beats: 3, cost_usd: 0.07, context_tokens: 118_000 };

  it('offers to turn the keep-alive off only while it keeps the cache warm', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(200, kept, 0.57));
    await openExplainer(await mount(threadWith('waiting_for_user_answer')));
    await act(async () => switchButton('Turn off for this thread')!.click());
    expect(switchCacheKeepAlive).toHaveBeenCalledWith('t1', 'off');
    expect(popover()).toBeNull();
  });

  it('offers no switch when nothing keeps the cache warm, or it stopped', async () => {
    for (const keepAlive of [NOTHING_PENDING, stopped]) {
      fetchCacheWarmth.mockResolvedValue(warm(200, keepAlive, 0.57));
      await openExplainer(await mount(threadWith('waiting_for_user_answer')));
      expect(switchButton('Turn off for this thread')).toBeUndefined();
      expect(switchButton('Turn back on')).toBeUndefined();
      document.body.innerHTML = '';
    }
  });

  it('names when it was turned off, when the cache goes cold, and what the run spent', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(240, turnedOff, 0.57));
    const host = await mount(threadWith('waiting_for_user_answer'));
    expect(host.textContent).toBe(`Cache warm · turned off · cold at ${coldAt(240)}`);
    expect(root()?.getAttribute('data-state')).toBe('turned_off');
    const text = await openExplainer(host);
    expect(text).toContain(`3 pings · $0.07 · turned off ${clock(OFF_AT)}.`);
    expect(text).toContain('118k tokens once, about $0.57');
  });

  it('turns back on while the cache is warm, and draws nothing once it is cold', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(30, turnedOff, null));
    const host = await mount(threadWith('waiting_for_user_answer'));
    await openExplainer(host);
    await act(async () => switchButton('Turn back on')!.click());
    expect(switchCacheKeepAlive).toHaveBeenCalledWith('t1', 'on');
    await act(async () => {
      vi.advanceTimersByTime(31_000);
    });
    expect(root()).toBeNull();
  });
});

describe('the cache-warm line after the keep-alive gave up', () => {
  it('says it stopped, and when the cache goes cold', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(180, stopped));
    const host = await mount(threadWith('waiting_for_user_answer'));
    expect(host.textContent).toBe(`Cache warm · stopped · cold at ${coldAt(180)}`);
    const text = await openExplainer(host);
    expect(text).toContain(`12 pings · $1.62 · stopped ${clock('2026-10-19T09:58:00Z')}, past break-even.`);
  });

  it('links the keep-alive setting', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(180, stopped));
    await openExplainer(await mount(threadWith('waiting_for_user_answer')));
    await clickLink('Settings → Models → Cache keep-alive');
    expect(menu.openCacheKeepAliveSetting).toHaveBeenCalledTimes(1);
  });
});

describe('the cache-warm line on a thread nothing keeps warm', () => {
  const reasons = [
    ['off', 'The cache keep-alive is off.'],
    ['codex', 'Codex sessions get no keep-alive: their cache cannot be kept warm this way.'],
    ['restarted', 'Lucidos restarted since the last request, so a ping would not match the cache.'],
    ['uncacheable_provider', "This model's provider has no prompt cache that a ping keeps warm."],
    ['nothing_pending', 'This thread waits only for your next message. Lucidos keeps a cache warm while a thread waits on a sub-thread, an event or a card.'],
  ] as const;

  it.each(reasons)('says why for %s', async (reason, sentence) => {
    fetchCacheWarmth.mockResolvedValue(warm(240, { state: 'not_kept', reason }));
    const host = await mount(threadWith('idle'));
    expect(host.textContent).toBe(`Cache warm · not kept · cold at ${coldAt(240)}`);
    const text = await openExplainer(host);
    expect(text).toContain('Nothing keeps it warm while you are away');
    expect(text).toContain(sentence);
  });

  it.each(reasons.filter(([reason]) => reason !== 'codex' && reason !== 'uncacheable_provider'))(
    'links the keep-alive setting for %s',
    async (reason) => {
      fetchCacheWarmth.mockResolvedValue(warm(240, { state: 'not_kept', reason }));
      await openExplainer(await mount(threadWith('idle')));
      await clickLink('Settings → Models → Cache keep-alive');
      expect(menu.openCacheKeepAliveSetting).toHaveBeenCalledTimes(1);
    },
  );

  it.each(['codex', 'uncacheable_provider'] as const)('drops the keep-alive sentence and link for %s', async (reason) => {
    fetchCacheWarmth.mockResolvedValue(warm(240, { state: 'not_kept', reason }));
    const text = await openExplainer(await mount(threadWith('idle')));
    expect(text).not.toContain('Settings → Models');
    expect(text).not.toContain('Set up the keep-alive in');
    expect(text).toContain('Hide this line in');
  });

  it.each(reasons)('links the setting that hides the line for %s', async (reason) => {
    fetchCacheWarmth.mockResolvedValue(warm(240, { state: 'not_kept', reason }));
    await openExplainer(await mount(threadWith('idle')));
    await clickLink('Settings → System → Debugging');
    expect(menu.openCacheWarmLineSetting).toHaveBeenCalledTimes(1);
  });

  it('always links the setting that hides the line', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(240));
    await openExplainer(await mount(threadWith('idle')));
    await clickLink('Settings → System → Debugging');
    expect(menu.openCacheWarmLineSetting).toHaveBeenCalledTimes(1);
  });
});

describe('the cache-warm line settings links', () => {
  const cases: [string, CacheKeepAliveStatus, number][] = [
    ['warming', kept, 2],
    ['stopped', stopped, 2],
    ['off', { state: 'not_kept', reason: 'off' }, 2],
    ['codex', { state: 'not_kept', reason: 'codex' }, 1],
    ['uncacheable_provider', { state: 'not_kept', reason: 'uncacheable_provider' }, 1],
    ['restarted', { state: 'not_kept', reason: 'restarted' }, 2],
    ['nothing_pending', NOTHING_PENDING, 2],
  ];

  it.each(cases)('gives each link its own paragraph with no stray period for %s', async (_name, keepAlive, linkCount) => {
    fetchCacheWarmth.mockResolvedValue(warm(240, keepAlive));
    await openExplainer(await mount(threadWith('idle')));
    const links = Array.from(popover()?.querySelectorAll('button.accent-link') ?? []);
    expect(links.length).toBe(linkCount);
    expect(new Set(links.map((link) => link.closest('p'))).size).toBe(linkCount);
    for (const link of links) {
      expect(link.nextSibling).toBeNull();
      expect(link.parentElement?.textContent?.trimEnd().endsWith('.')).toBe(false);
    }
    const walker = document.createTreeWalker(popover() as Node, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      expect((node.textContent ?? '').trim().startsWith('.')).toBe(false);
    }
  });
});

describe('the cache-warm line and a copy beat', () => {
  it('reads again when a cache keep-alive capture lands on the thread', async () => {
    fetchCacheWarmth.mockResolvedValueOnce(warm(20, kept)).mockResolvedValueOnce(warm(240, { ...kept, beats: 4 } as CacheKeepAliveStatus));
    const thread = threadWith('waiting_for_user_answer');
    const host = document.createElement('div');
    document.body.appendChild(host);
    await show(thread, host);
    expect(fetchCacheWarmth).toHaveBeenCalledTimes(1);

    handleEvent(
      new Map([['t1', thread]]), 't1', 1,
      { type: 'ContextCaptured', purpose: 'cache_keep_alive' } as unknown as ThreadEvent,
      '2026-10-19T10:00:00Z',
    );
    expect(thread.events.size).toBe(1);
    await show(thread, host);
    expect(fetchCacheWarmth).toHaveBeenCalledTimes(2);
  });
});

describe('the cache-warm line switch', () => {
  it('is on until the user turns it off', () => {
    expect(currentCacheWarmLine()).toBe(true);
  });

  it('draws nothing and reads nothing when the call site passes it off', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(240));
    const host = document.createElement('div');
    document.body.appendChild(host);
    await act(async () => {
      render(<CacheWarmLine threadId="t1" eligible={false} eventCount={0} />, host);
    });
    expect(root()).toBeNull();
    expect(fetchCacheWarmth).not.toHaveBeenCalled();
  });
});
