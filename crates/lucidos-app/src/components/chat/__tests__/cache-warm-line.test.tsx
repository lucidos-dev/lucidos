// @vitest-environment jsdom
/**
 * The *cache-warm line*: a card that closes the feed of a waiting thread while
 * its prompt cache is warm and no keep-alive card speaks for it. The engine
 * answers how long is left. The card goes at the cold time and re-reads when
 * the thread's status or events change.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type { CacheKeepAliveStatus } from '../../../generated/thread-event-wire';
import { answeringThreadIds } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import type { ThreadState } from '../../../store/thread-events';
import { applySummaryVersion } from '../../../store/thread-events/thread-meta';
import { currentCacheCards } from '../../../store/actions/preferences';
import type { CacheWarmth } from '../../../api/client/successor';
import { handleEvent, type ThreadEvent } from '../../../store/thread-events';
import { formatShortTime } from '../../../utils/formatTime';
import { cacheWarmLineEligible, CacheWarmLine } from '../CacheKeepAlive';

const fetchCacheWarmth = vi.fn<() => Promise<CacheWarmth>>();
const switchCacheKeepAlive = vi.fn<(threadId: string, state: 'off' | 'on') => Promise<void>>();

const menu = vi.hoisted(() => ({ openCacheKeepAliveSetting: vi.fn(), openCacheCardsSetting: vi.fn() }));

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

const warm = (remaining_secs: number, keep_alive: CacheKeepAliveStatus = NOTHING_PENDING): CacheWarmth => ({
  warmth: 'warm',
  remaining_secs,
  keep_alive,
});
const COLD: CacheWarmth = { warmth: 'cold', remaining_secs: null, keep_alive: null };
const unknown = (keep_alive: CacheKeepAliveStatus = NOTHING_PENDING): CacheWarmth => ({
  warmth: 'unknown',
  remaining_secs: null,
  keep_alive,
});

const kept: CacheKeepAliveStatus = {
  state: 'warming', since: '2026-10-19T09:50:00Z', beats: 3, budget_beats: 25, beat_every_secs: 230, cost_usd: 0.412,
  context_tokens: 182_000, give_up_at: '2026-10-19T10:20:00Z',
};
const stopped: CacheKeepAliveStatus = {
  state: 'stopped', at: '2026-10-19T09:58:00Z', beats: 12, cost_usd: 1.62, context_tokens: 182_000,
};
const turnedOff: CacheKeepAliveStatus = {
  state: 'turned_off', at: '2026-10-19T09:59:00Z', beats: 3, cost_usd: 0.07, context_tokens: 118_000,
};
/** The card's detail for a cache that goes cold `secs` after the read. */
const warmUntil = (secs: number) => `Warm until ${formatShortTime(new Date(NOW + secs * 1000))}`;
const detailOf = (host: HTMLElement) => host.querySelector('.event-row-detail')?.textContent;

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
  menu.openCacheCardsSetting.mockReset();
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

describe('the cache-warm card', () => {
  it('says when the warm cache of an idle thread goes cold, and that nothing keeps it', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(240));
    const host = await mount(threadWith('idle'));
    expect(fetchCacheWarmth).toHaveBeenCalledWith('t1');
    expect(host.querySelector('.event-name')?.textContent).toBe('Prompt cache');
    expect(host.querySelector('.event-row-state')?.textContent).toBe('Not kept');
    expect(detailOf(host)).toBe(warmUntil(240));
    expect(root()?.getAttribute('data-state')).toBe('not_kept');
  });

  it('says the lifetime is unknown when no evidence gives it, and keeps drawing', async () => {
    fetchCacheWarmth.mockResolvedValue(unknown());
    const host = await mount(threadWith('idle'));
    expect(host.querySelector('.event-row-state')?.textContent).toBe('Not kept');
    expect(detailOf(host)).toBe('Lifetime unknown');
    await act(async () => {
      vi.advanceTimersByTime(24 * 3600 * 1000);
    });
    expect(detailOf(host)).toBe('Lifetime unknown');
  });

  it('names no cache lifetime in the explainer when none is known', async () => {
    fetchCacheWarmth.mockResolvedValue(unknown({ state: 'not_kept', reason: 'unknown_ttl' }));
    const text = await openExplainer(await mount(threadWith('idle')));
    expect(text).toContain('Lucidos does not know how long this cache lives');
    expect(text).not.toMatch(/minute|hour/i);
  });

  it('is the one card component the transcript draws', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(240));
    await mount(threadWith('idle'));
    expect(root()?.classList.contains('event-row')).toBe(true);
    expect(root()?.getAttribute('data-kind')).toBe('cache');
  });

  it('never calls a cache nothing keeps kept warm, in the title or the text', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(240));
    const text = await openExplainer(await mount(threadWith('idle')));
    expect(popover()?.getAttribute('aria-label')).toBe('Prompt cache');
    expect(text).not.toContain('Lucidos pings it');
    expect(text).toContain('A turn while it is warm reads the context cheaply');
  });

  it('shows a time, not a countdown, and reads once', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(90));
    const host = await mount(threadWith('idle'));
    await act(async () => {
      vi.advanceTimersByTime(40_000);
    });
    expect(detailOf(host)).toBe(warmUntil(90));
    expect(fetchCacheWarmth).toHaveBeenCalledTimes(1);
  });

  it('goes when the cache goes cold', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(5));
    await mount(threadWith('idle'));
    expect(root()).not.toBeNull();
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
    expect(detailOf(host)).toBe(warmUntil(240));
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

  it.each([
    ['warming', kept],
    ['stopped', stopped],
    ['turned_off', turnedOff],
  ] as const)('gives way to the keep-alive card of a %s run', async (_state, keepAlive) => {
    fetchCacheWarmth.mockResolvedValue(warm(240, keepAlive));
    await mount(threadWith('idle', keepAlive));
    expect(root()).toBeNull();
  });

  it('draws once beside a warming card on a thread parked on a question', async () => {
    // Mid-turn the shown status is null while the warming card stays open, so
    // only the engine's read can tell that the card already speaks.
    fetchCacheWarmth.mockResolvedValue(warm(240, kept));
    await mount(threadWith('waiting_for_user_answer', kept));
    expect(root()).toBeNull();
  });

  it('draws beside a not-kept status, which has no card', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(240, { state: 'not_kept', reason: 'no_wake_children' }));
    const host = await mount(threadWith('idle', { state: 'not_kept', reason: 'no_wake_children' }));
    expect(detailOf(host)).toBe(warmUntil(240));
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
    expect(detailOf(host)).toBe(warmUntil(240));
    await act(async () => {
      answerSecond(warm(300));
      await Promise.resolve();
    });
    expect(detailOf(host)).toBe(warmUntil(300));
  });

  it('reads again when a new turn has landed, and when the thread goes idle', async () => {
    fetchCacheWarmth.mockResolvedValueOnce(warm(30)).mockResolvedValueOnce(warm(300));
    const host = document.createElement('div');
    document.body.appendChild(host);
    await show(threadWith('idle'), host);
    expect(detailOf(host)).toBe(warmUntil(30));

    await show(threadWith('running'), host);
    expect(root()).toBeNull();

    await show(threadWith('idle'), host);
    expect(fetchCacheWarmth).toHaveBeenCalledTimes(2);
    expect(detailOf(host)).toBe(warmUntil(300));
  });

  it('goes when a thread updated in place starts running', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(240));
    const thread = threadWith('idle');
    const host = await mount(thread);
    expect(detailOf(host)).toBe(warmUntil(240));
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
    expect(detailOf(host)).toBe(warmUntil(300));
    await act(async () => answerOld(warm(10)));
    expect(detailOf(host)).toBe(warmUntil(300));
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

describe('the cache-warm card on a thread nothing keeps warm', () => {
  const reasons = [
    ['off', 'The cache keep-alive is off.'],
    ['codex', 'Codex sessions get no keep-alive: their cache cannot be kept warm this way.'],
    ['restarted', 'Lucidos restarted since the last request, so a ping would not match the cache.'],
    ['uncacheable_provider', 'Lucidos cannot safely prefill this prompt cache.'],
    ['provider_paused', 'Pings on this provider are paused after a failed ping.'],
    ['prefill_paused', 'Prefill pings are temporarily paused.'],
    ['pings_paused', "Pings to this model's provider kept missing the cache, so they pause for a while."],
    ['cache_expired', 'The cache went cold before a ping reached it, so a ping would pay to rewrite it.'],
    ['model_changed', 'The model or reasoning effort changed, so this cache cannot be kept warm.'],
    ['beat_missed', 'A ping missed the cache, so this wait is not kept warm.'],
    ['past_give_up_time', 'The wait passed its give-up time before the first ping was due, so no ping went out.'],
    ['nothing_pending', 'This thread waits only for your next message. Lucidos keeps a cache warm while a thread waits on a sub-thread, an event or a card.'],
  ] as const;

  it.each(reasons)('says why for %s', async (reason, sentence) => {
    fetchCacheWarmth.mockResolvedValue(warm(240, { state: 'not_kept', reason }));
    const host = await mount(threadWith('idle'));
    expect(detailOf(host)).toBe(warmUntil(240));
    const text = await openExplainer(host);
    expect(text).toContain('Nothing keeps it warm while you are away');
    expect(text).toContain(sentence);
  });

  it('says when paused pings start again', async () => {
    const until = new Date(Date.now() + 40 * 60_000).toISOString();
    fetchCacheWarmth.mockResolvedValue(warm(240, { state: 'not_kept', reason: 'pings_paused', until }));
    const text = await openExplainer(await mount(threadWith('idle')));
    expect(text).toContain(`Pings to this model's provider kept missing the cache, so they pause until ${formatShortTime(new Date(until))}.`);
  });

  const settingCannotApply = ['codex', 'uncacheable_provider', 'provider_paused', 'prefill_paused', 'pings_paused'] as const;

  it.each(reasons.filter(([reason]) => !(settingCannotApply as readonly string[]).includes(reason)))(
    'links the keep-alive setting for %s',
    async (reason) => {
      fetchCacheWarmth.mockResolvedValue(warm(240, { state: 'not_kept', reason }));
      await openExplainer(await mount(threadWith('idle')));
      await clickLink('Settings → Models → Cache keep-alive');
      expect(menu.openCacheKeepAliveSetting).toHaveBeenCalledTimes(1);
    },
  );

  it.each(settingCannotApply)('drops the keep-alive sentence and link for %s', async (reason) => {
    fetchCacheWarmth.mockResolvedValue(warm(240, { state: 'not_kept', reason }));
    const text = await openExplainer(await mount(threadWith('idle')));
    expect(text).not.toContain('Settings → Models');
    expect(text).not.toContain('Set up the keep-alive in');
    expect(text).toContain('Hide cache cards in');
  });

  it.each(reasons)('links the setting that hides the line for %s', async (reason) => {
    fetchCacheWarmth.mockResolvedValue(warm(240, { state: 'not_kept', reason }));
    await openExplainer(await mount(threadWith('idle')));
    await clickLink('Settings → System → Debugging');
    expect(menu.openCacheCardsSetting).toHaveBeenCalledTimes(1);
  });

  it('always links the setting that hides the cache cards', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(240));
    await openExplainer(await mount(threadWith('idle')));
    await clickLink('Settings → System → Debugging');
    expect(menu.openCacheCardsSetting).toHaveBeenCalledTimes(1);
  });
});

describe('the cache-warm card settings links', () => {
  const cases: [string, CacheKeepAliveStatus, number][] = [
    ['off', { state: 'not_kept', reason: 'off' }, 2],
    ['codex', { state: 'not_kept', reason: 'codex' }, 1],
    ['uncacheable_provider', { state: 'not_kept', reason: 'uncacheable_provider' }, 1],
    ['provider_paused', { state: 'not_kept', reason: 'provider_paused' }, 1],
    ['prefill_paused', { state: 'not_kept', reason: 'prefill_paused' }, 1],
    ['restarted', { state: 'not_kept', reason: 'restarted' }, 2],
    ['past_give_up_time', { state: 'not_kept', reason: 'past_give_up_time' }, 2],
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

  it('words the give-up setting for a wait that passed its give-up time', async () => {
    fetchCacheWarmth.mockResolvedValue(warm(240, { state: 'not_kept', reason: 'past_give_up_time' }));
    const text = await openExplainer(await mount(threadWith('idle')));
    expect(text).toContain('Change when it gives up in');
  });
});

describe('the cache-warm card and a copy beat', () => {
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

describe('the cache cards switch', () => {
  it('is off until the user turns it on', () => {
    expect(currentCacheCards()).toBe(false);
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
