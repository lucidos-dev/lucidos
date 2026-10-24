// @vitest-environment jsdom
/**
 * The surfaces of the *cache keep-alive status* that read the thread: the card a
 * keep-alive run leaves in the transcript, and the waiting panel's Prompt cache
 * section. Both speak the one fact line (`keepAliveFacts`).
 */
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import type { CacheKeepAliveNotKeptReason, CacheKeepAliveStatus } from '../../../generated/thread-event-wire';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import { threadMap } from '../../../store/store';
import { formatShortTime } from '../../../utils/formatTime';
import { keepAliveFacts, KeepAliveRow, KeepAliveSection, shownKeepAliveStatus } from '../CacheKeepAlive';
import { waitingPanelBody } from '../WaitingPanel';
import { describeInitiator } from '../ChatExchange';
import type { StoredEvent } from '../../../store/thread-events';
import { ENGINE_LABEL } from '../../../store/thread-events';

vi.mock('../../../store/actions/threads', () => ({ focusThreadOrBootstrap: vi.fn() }));

const switchCacheKeepAlive = vi.hoisted(() => vi.fn<(threadId: string, state: 'off' | 'on') => Promise<void>>());

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  switchCacheKeepAlive,
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

function mount(node: preact.ComponentChild): HTMLElement {
  const root = document.createElement('div');
  document.body.appendChild(root);
  render(node, root);
  return root;
}

afterEach(() => {
  document.body.innerHTML = '';
  vi.useRealTimers();
});

describe('the one fact line', () => {
  it('counts against the budget while the run is open, and says when it stops', () => {
    expect(keepAliveFacts(warming())).toBe(`12 of 25 pings · $0.31 · until ${clock(GIVE_UP)}`);
  });

  it('drops the budget once the run is closed', () => {
    expect(keepAliveFacts(warming({ beats: 3 }), false)).toBe('3 pings · $0.31');
  });

  it('says when the pings stopped, and why', () => {
    expect(keepAliveFacts(stopped)).toBe(`25 pings · $0.64 · stopped ${clock(GIVE_UP)}, past break-even`);
  });

  it('gives an ended run its span and what the warm cache saved', () => {
    expect(keepAliveFacts(ended)).toBe(`23 pings · $0.29 · ${clock('2026-10-19T09:00:00Z')}–${clock('2026-10-19T09:34:00Z')} · saved about $0.57`);
    const cold = { ...ended, saved_usd: undefined } as CacheKeepAliveStatus;
    expect(keepAliveFacts(cold)).not.toContain('saved');
  });

  it('says when the user turned it off', () => {
    expect(keepAliveFacts(turnedOff)).toBe(`9 pings · $0.12 · turned off ${clock('2026-10-19T09:38:00Z')}`);
  });

  it('has none for a thread it does not keep warm', () => {
    expect(keepAliveFacts({ state: 'not_kept', reason: 'restarted' })).toBeNull();
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
    expect(root.textContent).toContain(`12 of 25 pings · $0.31 · until ${clock(GIVE_UP)}`);
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
    expect(root.textContent).toContain('3 pings · $0.31');
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
    expect(root.textContent).toContain(keepAliveFacts(stopped)!);
  });

  it('says how an ended run went, and is closed', () => {
    const root = mountCard(ended, threadWith(ended));
    expect(root.textContent).toContain('Ended');
    expect(root.textContent).toContain(keepAliveFacts(ended)!);
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

  it('draws nothing for a thread it does not keep warm', () => {
    const root = mountCard({ state: 'not_kept', reason: 'no_wake_children' });
    expect(root.querySelector('[data-role="cache-keep-alive"]')).toBeNull();
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
  it('says how far into its budget the keep-alive is, with the cards own line', () => {
    const root = mount(<KeepAliveSection threadId="t1" status={warming()} />);
    expect(root.textContent).toContain(`Cache kept warm · ${keepAliveFacts(warming())}`);
    expect((root.querySelector('.keep-alive-meter > span') as HTMLElement).style.width).toBe('48%');
  });

  it('says it stopped at break-even', () => {
    const root = mount(<KeepAliveSection threadId="t1" status={stopped} />);
    expect(root.textContent).toContain(`Cache warm · stopped ${clock(GIVE_UP)}`);
    expect(root.textContent).toContain('25 pings, $0.64. More pings would cost more than rereading 262k tokens once.');
    expect(root.querySelector('.keep-alive-meter')).toBeNull();
  });

  it('says why it keeps nothing warm, from the one reason list', () => {
    const detail = (reason: CacheKeepAliveNotKeptReason) =>
      mount(<KeepAliveSection threadId="t1" status={{ state: 'not_kept', reason }} />).textContent;
    expect(detail('no_wake_children')).toContain('These sub-threads finish without waking this thread');
    expect(detail('children_awaiting_user')).toContain('The sub-threads are waiting on you.');
    expect(detail('uncacheable_provider')).toContain("This model's provider has no prompt cache that a ping keeps warm.");
    expect(detail('restarted')).toContain('Lucidos restarted since the last request');
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

  it('says the user turned it off, and offers to turn it back on', () => {
    switchCacheKeepAlive.mockResolvedValue(undefined);
    const root = mount(<KeepAliveSection threadId="t1" status={turnedOff} />);
    expect(root.textContent).toContain(`Cache warm · turned off ${clock('2026-10-19T09:38:00Z')}`);
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
