// @vitest-environment jsdom
/**
 * The surfaces of the *cache keep-alive status* that read the thread: the card a
 * keep-alive run leaves in the transcript, and the waiting panel's Prompt cache
 * section. Both speak the run's sentences (`keepAliveSentence`), and the
 * explainer shows the keep-alive math.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type { CacheWarmth } from '../../../api/client/successor';
import type { CacheKeepAliveNotKeptReason, CacheKeepAliveStatus } from '../../../generated/thread-event-wire';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import { threadMap } from '../../../store/store';
import { formatShortTime } from '../../../utils/formatTime';
import { keepAliveMath, keepAliveSentence, KeepAliveRow, KeepAliveSection, shownKeepAliveStatus } from '../CacheKeepAlive';
import { waitingPanelBody } from '../WaitingPanel';
import { describeInitiator } from '../ChatExchange';
import type { StoredEvent } from '../../../store/thread-events';
import { ENGINE_LABEL } from '../../../store/thread-events';

vi.mock('../../../store/actions/threads', () => ({ focusThreadOrBootstrap: vi.fn() }));

const switchCacheKeepAlive = vi.hoisted(() => vi.fn<(threadId: string, state: 'off' | 'on') => Promise<void>>());
const fetchCacheWarmth = vi.hoisted(() => vi.fn<(threadId: string) => Promise<CacheWarmth>>());

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  switchCacheKeepAlive,
  fetchCacheWarmth,
}));

const GIVE_UP = '2026-10-19T10:30:00Z';

const warming = (over: Partial<Extract<CacheKeepAliveStatus, { state: 'warming' }>> = {}): CacheKeepAliveStatus => ({
  state: 'warming',
  since: '2026-10-19T09:00:00Z',
  beats: 12,
  budget_beats: 25,
  beat_every_secs: 230,
  cost_usd: 0.31,
  context_tokens: 262_000,
  next_beat_at: '2026-10-19T10:03:00Z',
  give_up_at: GIVE_UP,
  ...over,
});

const stopped: CacheKeepAliveStatus = {
  state: 'stopped', at: GIVE_UP, beats: 25, cost_usd: 0.64, context_tokens: 262_000,
};

const ended: CacheKeepAliveStatus = {
  state: 'ended', since: '2026-10-19T09:00:00Z', at: '2026-10-19T09:34:00Z', beats: 23, cost_usd: 0.29, saved_usd: 0.57,
};

const turnedOff: CacheKeepAliveStatus = {
  state: 'turned_off', at: '2026-10-19T09:38:00Z', beats: 9, cost_usd: 0.12, context_tokens: 118_000,
};

const clock = (iso: string) => formatShortTime(new Date(iso));

function threadWith(status: CacheKeepAliveStatus | null, threadStatus: 'idle' | 'running' | 'waiting_for_user_answer' = 'idle') {
  return makeThreadState('t1', { meta: { status: threadStatus, cacheKeepAliveStatus: status } });
}

const mounted: HTMLElement[] = [];

function mount(node: preact.ComponentChild): HTMLElement {
  const root = document.createElement('div');
  document.body.appendChild(root);
  render(node, root);
  mounted.push(root);
  return root;
}

afterEach(() => {
  // Unmount, so an open explainer leaves the overlay stack.
  act(() => {
    for (const root of mounted.splice(0)) render(null, root);
  });
  document.body.innerHTML = '';
  vi.useRealTimers();
});

/** A warming run with prices and odds: a ping $0.06, the reread $0.74, beats
 *  every 4.5 minutes, a cap of 2. Of 10 past waits, 5 ended before the first
 *  ping, 2 after one, 1 after two, and 2 outlasted the cap. */
const priced = warming({
  beats: 1, budget_beats: 2, beat_every_secs: 270, cost_usd: 0.06, context_tokens: 118_000,
  ping_usd: 0.059, reread_usd: 0.7375, odds: { samples: 10, slots: [5, 2, 1], after_cap: 2 },
});

describe('the run sentences', () => {
  it('says what an open run spent and until when it keeps warm', () => {
    expect(keepAliveSentence(warming())).toBe(`Spent $0.31 so far. Warm until ${clock(GIVE_UP)}.`);
  });

  it('adds what keeping it warm should save, once the engine priced the odds', () => {
    expect(keepAliveSentence(priced)).toBe(`Spent $0.06 so far. Expected to save $0.17. Warm until ${clock(GIVE_UP)}.`);
  });

  it('names the expected wake the plan follows, and who expects it', () => {
    const wake = '2026-10-19T10:05:00Z';
    expect(keepAliveSentence(warming({ expected_wake: { at: wake, source: 'agent' } }))).toBe(
      `Spent $0.31 so far. Warm until ${clock(GIVE_UP)}. The agent expects a wake about ${clock(wake)}.`,
    );
    expect(keepAliveSentence(warming({ expected_wake: { at: wake, source: 'decision' } }))).toBe(
      `Spent $0.31 so far. Warm until ${clock(GIVE_UP)}. Lucidos estimates a wake about ${clock(wake)}.`,
    );
  });

  it('counts the pings once the run is closed', () => {
    expect(keepAliveSentence(warming({ beats: 3 }), false)).toBe('Spent $0.31 on 3 pings.');
  });

  it('says when the pings stopped, and why', () => {
    expect(keepAliveSentence(stopped)).toBe(`Spent $0.64 on 25 pings. Stopped at ${clock(GIVE_UP)}, the end of its plan.`);
  });

  it('gives an ended run its span and what the warm cache saved', () => {
    expect(keepAliveSentence(ended)).toBe(`Spent $0.29 on 23 pings, ${clock('2026-10-19T09:00:00Z')} to ${clock('2026-10-19T09:34:00Z')}. Saved about $0.57.`);
    const cold = { ...ended, saved_usd: undefined } as CacheKeepAliveStatus;
    expect(keepAliveSentence(cold)).not.toContain('Saved');
  });

  it('says when the user turned it off', () => {
    expect(keepAliveSentence(turnedOff)).toBe(`Spent $0.12 on 9 pings. Turned off at ${clock('2026-10-19T09:38:00Z')}.`);
  });

  it('has none for a thread it does not keep warm', () => {
    expect(keepAliveSentence({ state: 'not_kept', reason: 'restarted' })).toBeNull();
  });
});

describe('the keep-alive math', () => {
  it('prices each slot for both choices, and a reply after the cap pays both', () => {
    const math = keepAliveMath(priced)!;
    expect(math.rows.map((row) => [row.when, row.chance, row.keepWarm, row.noKeepAlive])).toEqual([
      ['0–4.5 min', 0.5, 0, 0],
      ['4.5–9 min', 0.2, 0.059, 0.7375],
      ['9–13.5 min', 0.1, 0.118, 0.7375],
      ['after 13.5 min', 0.2, 0.118 + 0.7375, 0.7375],
    ]);
    expect(math.keepWarm).toBeCloseTo(0.2 * 0.059 + 0.1 * 0.118 + 0.2 * (0.118 + 0.7375));
    expect(math.noKeepAlive).toBeCloseTo(0.5 * 0.7375);
  });

  it('never reads a slot that past waits ended in as empty', async () => {
    const rare = warming({ ...priced, odds: { samples: 1000, slots: [990, 4, 0], after_cap: 6 } } as Partial<Extract<CacheKeepAliveStatus, { state: 'warming' }>>);
    threadMap.value = new Map([['t1', threadWith(rare)]]);
    const root = mount(<KeepAliveRow threadId="t1" status={rare} />);
    await act(async () => {
      root.querySelector<HTMLButtonElement>('button[aria-label^="About"]')!.click();
    });
    await act(async () => {
      document.querySelector<HTMLButtonElement>('.keep-alive-math-toggle')!.click();
    });
    const chances = [...document.querySelectorAll('[data-role="keep-alive-math"] tbody td:nth-child(2)')].map((td) => td.textContent);
    expect(chances).toEqual(['99%', '<1%', '0%', '1%']);
    threadMap.value = new Map();
  });

  it('has none without odds, an interval or prices, or for a run that kept nothing', () => {
    expect(keepAliveMath(warming())).toBeNull();
    expect(keepAliveMath({ ...priced, odds: undefined } as CacheKeepAliveStatus)).toBeNull();
    expect(keepAliveMath({ ...priced, reread_usd: undefined } as CacheKeepAliveStatus)).toBeNull();
    expect(keepAliveMath({ ...priced, beat_every_secs: undefined, state: 'stopped' } as unknown as CacheKeepAliveStatus)).toBeNull();
    expect(keepAliveMath(stopped)).toBeNull();
    expect(keepAliveMath({ state: 'not_kept', reason: 'restarted' })).toBeNull();
  });
});

/** The closed states carry the cap's odds and interval, as the engine copies them. */
const withMath = { ping_usd: 0.059, reread_usd: 0.7375, beat_every_secs: 270, odds: { samples: 10, slots: [5, 2, 1], after_cap: 2 } };
const closed = (status: CacheKeepAliveStatus): CacheKeepAliveStatus => ({ ...status, ...withMath }) as CacheKeepAliveStatus;

describe('the final score of a closed run', () => {
  it('prices a stopped run by the reread it pays anyway, and marks the last row', () => {
    const math = keepAliveMath(closed(stopped))!;
    expect(math.score!.keepWarm).toBeCloseTo(0.64 + 0.7375);
    expect(math.score!.noKeepAlive).toBeCloseTo(0.7375);
    expect(math.landed).toBe(math.rows.length - 1);
  });

  it('scores an ended run that read a warm cache by what it saved, in the row of its pings', () => {
    const math = keepAliveMath(closed({ ...ended, beats: 2, cost_usd: 0.12, context_tokens: 118_000 }))!;
    expect(math.score).toEqual({ keepWarm: 0.12, noKeepAlive: 0.57 });
    expect(math.landed).toBe(2);
  });

  it('scores an ended run on a cold cache like a stopped one, with no row to mark', () => {
    const math = keepAliveMath(closed({ ...ended, beats: 1, cost_usd: 0.06, saved_usd: undefined, context_tokens: 118_000 } as CacheKeepAliveStatus))!;
    expect(math.score!.keepWarm).toBeCloseTo(0.06 + 0.7375);
    expect(math.score!.noKeepAlive).toBeCloseTo(0.7375);
    expect(math.landed).toBeNull();
  });

  it('scores a run that never pinged at zero both ways, in the first row', () => {
    const math = keepAliveMath(closed({ ...ended, beats: 0, cost_usd: 0, saved_usd: undefined, context_tokens: 118_000 } as CacheKeepAliveStatus))!;
    expect(math.score).toEqual({ keepWarm: 0, noKeepAlive: 0 });
    expect(math.landed).toBe(0);
  });

  it('scores a run the user turned off by the reread, with no row to mark', () => {
    const math = keepAliveMath(closed(turnedOff))!;
    expect(math.score!.keepWarm - math.score!.noKeepAlive).toBeCloseTo(0.12);
    expect(math.landed).toBeNull();
  });

  it('leaves an open run without one', () => {
    expect(keepAliveMath(priced)!.score).toBeNull();
    expect(keepAliveMath(priced)!.landed).toBeNull();
  });
});

describe('the keep-alive card', () => {
  function mountCard(card: CacheKeepAliveStatus, thread = threadWith(card)) {
    threadMap.value = new Map([['t1', thread]]);
    return mount(<KeepAliveRow threadId="t1" status={card} />);
  }

  afterEach(() => {
    threadMap.value = new Map();
  });

  it('counts the pings and the cost, with a meter and the switch, while its run is open', () => {
    const root = mountCard(warming());
    expect(root.textContent).toContain('Cache kept warm');
    expect(root.textContent).toContain('Warming');
    expect(root.textContent).toContain(`Spent $0.31 so far. Warm until ${clock(GIVE_UP)}.`);
    expect((root.querySelector('.keep-alive-meter > span') as HTMLElement).style.width).toBe('48%');
    expect(root.querySelector('[data-role="cache-keep-alive-switch"]')?.textContent).toBe('Turn off for this thread');
  });

  it('stays live while its thread is parked on a question', () => {
    const root = mountCard(warming(), threadWith(warming(), 'waiting_for_user_answer'));
    expect(root.textContent).toContain('Warming');
    expect(root.querySelector('.keep-alive-meter')).not.toBeNull();
  });

  it('keeps its record after the run closes, without a meter or a switch', () => {
    const closed = warming({ beats: 3 });
    const root = mountCard(closed, threadWith(null));
    expect(root.textContent).toContain('Spent $0.31 on 3 pings.');
    expect(root.textContent).toContain('Ended');
    expect(root.querySelector('.keep-alive-meter')).toBeNull();
    expect(root.querySelector('[data-role="cache-keep-alive-switch"]')).toBeNull();
    expect(root.querySelector('[data-role="cache-keep-alive"]')?.getAttribute('data-state')).toBe('warming');
  });

  it('is not open for an older wait of the same thread', () => {
    const older = warming({ since: '2026-10-19T08:00:00Z' });
    const root = mountCard(older, threadWith(warming()));
    expect(root.querySelector('.keep-alive-meter')).toBeNull();
  });

  it('is not open while the thread runs', () => {
    const root = mountCard(warming(), threadWith(warming(), 'running'));
    expect(root.querySelector('.keep-alive-meter')).toBeNull();
  });

  it('says when the pings stopped', () => {
    const root = mountCard(stopped);
    expect(root.textContent).toContain('Stopped');
    expect(root.textContent).toContain(keepAliveSentence(stopped)!);
  });

  it('says how an ended run went, and is closed', () => {
    const root = mountCard(ended, threadWith(ended));
    expect(root.textContent).toContain('Ended');
    expect(root.textContent).toContain(keepAliveSentence(ended)!);
    expect(root.querySelector('[data-role="cache-keep-alive-switch"]')).toBeNull();
  });

  it('offers to turn it back on while the run is turned off', () => {
    switchCacheKeepAlive.mockResolvedValue(undefined);
    const root = mountCard(turnedOff);
    expect(root.textContent).toContain('Turned off');
    const button = root.querySelector<HTMLButtonElement>('[data-role="cache-keep-alive-switch"]');
    expect(button?.textContent).toBe('Turn back on');
    button!.click();
    expect(switchCacheKeepAlive).toHaveBeenCalledWith('t1', 'on');
  });

  it('turns off before the first ping, from the card, and says so', () => {
    switchCacheKeepAlive.mockResolvedValue(undefined);
    const first = warming({ beats: 0, cost_usd: 0 });
    const root = mountCard(first);
    expect(root.textContent).toContain('Spent $0.00 so far.');
    const button = root.querySelector<HTMLButtonElement>('[data-role="cache-keep-alive-switch"]');
    expect(button?.textContent).toBe('Turn off for this thread');
    button!.click();
    expect(switchCacheKeepAlive).toHaveBeenCalledWith('t1', 'off');
  });

  it('keeps the card of a run turned off before its first ping, which can turn back on', () => {
    switchCacheKeepAlive.mockResolvedValue(undefined);
    const off = { ...turnedOff, beats: 0, cost_usd: 0 } as CacheKeepAliveStatus;
    const root = mountCard(off);
    expect(root.textContent).toContain(`Spent $0.00 on 0 pings. Turned off at ${clock('2026-10-19T09:38:00Z')}, before the first ping.`);
    root.querySelector<HTMLButtonElement>('[data-role="cache-keep-alive-switch"]')!.click();
    expect(switchCacheKeepAlive).toHaveBeenCalledWith('t1', 'on');
  });

  it('says a run that ended before its first ping saved nothing', () => {
    const early = { ...ended, beats: 0, cost_usd: 0, saved_usd: undefined } as CacheKeepAliveStatus;
    expect(keepAliveSentence(early)).toBe(
      `Spent $0.00 on 0 pings, ${clock('2026-10-19T09:00:00Z')} to ${clock('2026-10-19T09:34:00Z')}. Ended before the first ping.`,
    );
  });

  it('draws nothing for a thread it does not keep warm', () => {
    const root = mountCard({ state: 'not_kept', reason: 'no_wake_children' });
    expect(root.querySelector('[data-role="cache-keep-alive"]')).toBeNull();
  });

  it.each([
    ['warming', warming(), 'Cache kept warm'],
    ['ended', ended, 'Cache kept warm'],
    ['stopped', stopped, 'Cache keep-alive'],
    ['turned_off', turnedOff, 'Cache keep-alive'],
  ] as const)('claims the cache kept warm only for a %s run: chip %s', (_state, card, chip) => {
    const root = mountCard(card, threadWith(card));
    expect(root.querySelector('.event-name')?.textContent).toBe(chip);
    expect(root.querySelector(`button[aria-label="About ${chip}"]`)).not.toBeNull();
  });

  it('explains itself with the one shared explainer, not a tooltip', () => {
    const root = mountCard(warming());
    expect(root.querySelector('.event-name')?.getAttribute('data-tooltip')).toBeNull();
    expect(root.querySelector('button[aria-label="About Cache kept warm"]')).not.toBeNull();
  });

  it('draws nothing as the status of a running thread, or of an ended run', () => {
    expect(shownKeepAliveStatus(threadWith(warming(), 'running'))).toBeNull();
    expect(shownKeepAliveStatus(threadWith(ended))).toBeNull();
    expect(shownKeepAliveStatus(threadWith(warming()))?.state).toBe('warming');
  });
});

describe('the keep-alive card in the transcript', () => {
  it('is drawn as an engine card by its exchange', () => {
    const userEvent = { type: 'CacheKeepAliveStatusChanged', status: warming() } as unknown as StoredEvent;
    const descriptor = describeInitiator({ userEvent, userSeq: 5, steps: [] }, '', [], 't1');
    expect(descriptor.variant).toBe('system');
    expect(descriptor.label).toBe(ENGINE_LABEL);
    expect(descriptor.details).toBeTruthy();
  });
});

describe('the Prompt cache section', () => {
  const NOW = Date.parse('2026-10-19T10:31:00Z');
  const COLD: CacheWarmth = { warmth: 'cold', remaining_secs: null, keep_alive: null };
  const warmFor = (secs: number, status: CacheKeepAliveStatus): CacheWarmth =>
    ({ warmth: 'warm', remaining_secs: secs, keep_alive: status });
  const headline = (root: HTMLElement) => root.querySelector('.waiting-panel-cache-line')?.textContent;

  beforeEach(() => {
    fetchCacheWarmth.mockReset();
    fetchCacheWarmth.mockResolvedValue(COLD);
  });

  /** Mount the section, then let the warmth read land. */
  async function mountRead(status: CacheKeepAliveStatus): Promise<HTMLElement> {
    let root!: HTMLElement;
    await act(async () => {
      root = mount(<KeepAliveSection threadId="t1" status={status} />);
    });
    await act(async () => {
      await Promise.resolve();
    });
    return root;
  }

  it('says what the keep-alive spent, with the cards own sentence', () => {
    const root = mount(<KeepAliveSection threadId="t1" status={warming()} />);
    expect(headline(root)).toBe(keepAliveSentence(warming()));
    expect((root.querySelector('.keep-alive-meter > span') as HTMLElement).style.width).toBe('48%');
    expect(fetchCacheWarmth).not.toHaveBeenCalled();
  });

  it('says it stopped at the reread cap', async () => {
    const root = await mountRead(stopped);
    expect(root.textContent).toContain('A reply after the cache goes cold rereads 262k tokens.');
    expect(root.querySelector('.keep-alive-meter')).toBeNull();
  });

  it('never calls a cold cache warm', async () => {
    const root = await mountRead(stopped);
    expect(fetchCacheWarmth).toHaveBeenCalledWith('t1');
    expect(headline(root)).toBe(`${keepAliveSentence(stopped)} The cache is cold.`);
  });

  it('says the lifetime is unknown when no evidence gives it', async () => {
    const unknownTtl: CacheKeepAliveStatus = { state: 'not_kept', reason: 'unknown_ttl' };
    fetchCacheWarmth.mockResolvedValue({ warmth: 'unknown', remaining_secs: null, keep_alive: unknownTtl });
    const root = await mountRead(unknownTtl);
    expect(headline(root)).toBe('Not kept Its lifetime is unknown.');
    expect(root.querySelector('.waiting-panel-cache-detail')?.textContent).toBe(
      'Lucidos does not know how long this cache lives, so it cannot time a ping.',
    );
  });

  it('says until when a stopped run leaves the cache warm, and turns cold on time', async () => {
    vi.useFakeTimers({ now: NOW, toFake: ['Date', 'setInterval', 'clearInterval'] });
    fetchCacheWarmth.mockResolvedValue(warmFor(120, stopped));
    const root = await mountRead(stopped);
    expect(headline(root)).toBe(`${keepAliveSentence(stopped)} Warm until ${formatShortTime(new Date(NOW + 120_000))}.`);
    await act(async () => {
      vi.advanceTimersByTime(121_000);
    });
    expect(headline(root)).toBe(`${keepAliveSentence(stopped)} The cache is cold.`);
  });

  it('claims no warmth while a run that just stopped reads again', async () => {
    const root = await mountRead(warming());
    fetchCacheWarmth.mockReturnValue(new Promise(() => {}));
    await act(async () => {
      render(<KeepAliveSection threadId="t1" status={stopped} />, root);
    });
    expect(headline(root)).toBe(keepAliveSentence(stopped));
  });

  it('claims no warmth before the read lands', () => {
    fetchCacheWarmth.mockReturnValue(new Promise(() => {}));
    const root = mount(<KeepAliveSection threadId="t1" status={stopped} />);
    expect(headline(root)).toBe(keepAliveSentence(stopped));
  });

  it('says why it keeps nothing warm, from the one reason list', () => {
    const detail = (reason: CacheKeepAliveNotKeptReason) =>
      mount(<KeepAliveSection threadId="t1" status={{ state: 'not_kept', reason }} />).textContent;
    expect(detail('no_wake_children')).toContain('These sub-threads finish without waking this thread');
    expect(detail('children_awaiting_user')).toContain('The sub-threads are waiting on you.');
    expect(detail('uncacheable_provider')).toContain('Lucidos cannot safely prefill this prompt cache.');
    expect(detail('provider_paused')).toContain('Pings on this provider are paused after a failed ping.');
    expect(detail('prefill_paused')).toContain('Prefill pings are temporarily paused.');
    expect(detail('restarted')).toContain('Lucidos restarted since the last request');
    expect(detail('model_changed')).toContain('The model or reasoning effort changed');
    expect(detail('beat_missed')).toContain('A ping missed the cache');
    expect(detail('codex')).toContain('Codex sessions get no keep-alive');
  });

  it('carries the per-thread switch while it keeps the cache warm, and nowhere else', () => {
    switchCacheKeepAlive.mockResolvedValue(undefined);
    const root = mount(<KeepAliveSection threadId="t1" status={warming()} />);
    const button = root.querySelector<HTMLButtonElement>('[data-role="cache-keep-alive-switch"]');
    expect(button?.textContent).toBe('Turn off for this thread');
    expect(button?.className).toContain('action-btn-secondary');
    button!.click();
    expect(switchCacheKeepAlive).toHaveBeenCalledWith('t1', 'off');
    const quiet = mount(<KeepAliveSection threadId="t1" status={stopped} />);
    expect(quiet.querySelector('[data-role="cache-keep-alive-switch"]')).toBeNull();
  });

  it('says the user turned it off, and offers to turn it back on', async () => {
    switchCacheKeepAlive.mockResolvedValue(undefined);
    const root = await mountRead(turnedOff);
    expect(headline(root)).toBe(`${keepAliveSentence(turnedOff)} The cache is cold.`);
    const button = root.querySelector<HTMLButtonElement>('[data-role="cache-keep-alive-switch"]');
    expect(button?.textContent).toBe('Turn back on');
    button!.click();
    expect(switchCacheKeepAlive).toHaveBeenCalledWith('t1', 'on');
  });

  it('carries its label beside a single unlabelled list', () => {
    const child = makeThreadState('c1', { meta: { status: 'running', parentThreadId: 't1' } });
    const root = mount(<>{waitingPanelBody({
      threadId: 't1',
      waits: [],
      subThreads: { threads: [child], unresolved: 0 },
      keepAlive: warming(),
      onClose: () => {},
    })}</>);
    const labels = [...root.querySelectorAll('.waiting-panel-section-label')].map((el) => el.textContent);
    expect(labels).toEqual(['Prompt cache']);
  });
});

describe('the keep-alive explainer', () => {
  afterEach(() => {
    threadMap.value = new Map();
  });

  /** Open the card's explainer and return the document's text. */
  async function explain(status: CacheKeepAliveStatus): Promise<string> {
    threadMap.value = new Map([['t1', threadWith(status)]]);
    const root = mount(<KeepAliveRow threadId="t1" status={status} />);
    await act(async () => {
      root.querySelector<HTMLButtonElement>('button[aria-label^="About"]')!.click();
    });
    return document.body.textContent ?? '';
  }

  it('links the switch that hides the cache cards', async () => {
    expect(await explain(warming())).toContain('Hide cache cards in Settings → Models → Cache keep-alive');
  });

  it('leaves the switch to the card that opened it', async () => {
    await explain(warming());
    expect(document.querySelectorAll('[data-role="cache-keep-alive-switch"]')).toHaveLength(1);
    expect(document.querySelector('.explainer-body')).not.toBeNull();
    expect(document.querySelector('.explainer-body [data-role="cache-keep-alive-switch"]')).toBeNull();
  });

  it('sums the expected cost, with the slot table folded away', async () => {
    const text = await explain(priced);
    expect(text).toContain(`1 of 2 pings sent, $0.06 so far. Warm until ${clock(GIVE_UP)}.`);
    expect(text).toContain('Expected cost: keep warm $0.19, no keep-alive $0.37, so keeping it warm saves about $0.17.');
    const toggle = document.querySelector<HTMLButtonElement>('.keep-alive-math-toggle')!;
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    expect(document.querySelector('[data-role="keep-alive-math"]')).toBeNull();
    await act(async () => {
      toggle.click();
    });
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    const table = document.querySelector('[data-role="keep-alive-math"]')!;
    expect(table.textContent).toContain('after 13.5 min20%$0.86$0.74\u2212$0.12');
    expect(document.body.textContent).toContain('118k × $0.50/M = $0.06');
    expect(document.body.textContent).toContain('118k × $6.25/M = $0.74');
    expect(document.body.textContent).toContain('your last 10 waits like this');
  });

  it.each([
    ['a stopped run', closed(stopped), 'Final cost: keep warm $1.38, no keep-alive $0.74, so keeping it warm cost $0.64 more.'],
    ['an ended run', closed({ ...ended, beats: 2, cost_usd: 0.12, context_tokens: 118_000 }), 'Final cost: keep warm $0.12, no keep-alive $0.57, so keeping it warm saved $0.45.'],
    ['a run turned off', closed(turnedOff), 'Final cost: keep warm $0.86, no keep-alive $0.74, so keeping it warm cost $0.12 more.'],
  ])('gives %s its final score, and the table of its math', async (_label, status, summary) => {
    const text = await explain(status);
    expect(text).toContain(summary);
    expect(text).not.toContain('Expected cost');
    await act(async () => {
      document.querySelector<HTMLButtonElement>('.keep-alive-math-toggle')!.click();
    });
    expect(document.querySelector('[data-role="keep-alive-math"]')).not.toBeNull();
  });

  it('marks the table row a closed run landed in', async () => {
    await explain(closed({ ...ended, beats: 2, cost_usd: 0.12, context_tokens: 118_000 }));
    await act(async () => {
      document.querySelector<HTMLButtonElement>('.keep-alive-math-toggle')!.click();
    });
    const landed = [...document.querySelectorAll('[data-role="keep-alive-math"] tbody tr[data-landed="true"]')];
    expect(landed.map((row) => row.firstElementChild!.textContent)).toEqual(['9–13.5 min']);
  });

  it('prices the reread for a run with no odds', async () => {
    const text = await explain({ ...ended, saved_usd: undefined, context_tokens: 118_000, reread_usd: 0.7375 } as CacheKeepAliveStatus);
    expect(text).toContain('Rereading 118k tokens costs $0.74.');
    expect(document.querySelector('.keep-alive-math-toggle')).toBeNull();
  });

  it('says only what an ended run saved, not the reread again', async () => {
    const text = await explain({ ...ended, context_tokens: 118_000, reread_usd: 0.7375 } as CacheKeepAliveStatus);
    expect(text).toContain('Saved about $0.57.');
    expect(text).not.toContain('Rereading');
  });

  it('reads a status stored before prices without a price', async () => {
    const text = await explain(ended);
    expect(text).not.toMatch(/NaN|undefined|Rereading/);
  });
});

describe('no keep-alive surface joins facts with a dot', () => {
  const statuses: CacheKeepAliveStatus[] = [warming(), priced, stopped, ended, turnedOff, { state: 'not_kept', reason: 'restarted' }];

  beforeEach(() => {
    fetchCacheWarmth.mockReset();
    fetchCacheWarmth.mockResolvedValue({ warmth: 'warm', remaining_secs: 120, keep_alive: stopped });
  });

  afterEach(() => {
    threadMap.value = new Map();
  });

  it.each(statuses.map((status) => [status.state, status] as const))('the card, the panel and the explainer of a %s status', async (_state, status) => {
    threadMap.value = new Map([['t1', threadWith(status)]]);
    await act(async () => {
      mount(<KeepAliveRow threadId="t1" status={status} />);
      mount(<KeepAliveSection threadId="t1" status={status} />);
    });
    await act(async () => {
      for (const button of document.querySelectorAll<HTMLButtonElement>('button[aria-label^="About"]')) button.click();
    });
    await act(async () => {
      document.querySelector<HTMLButtonElement>('.keep-alive-math-toggle')?.click();
    });
    expect(document.body.textContent).toContain("The model holds this thread's context");
    expect(document.body.textContent).not.toContain(' · ');
  });
});
