// @vitest-environment jsdom
/**
 * The two surfaces of the *cache keep-alive status*: the card a keep-alive run
 * leaves in the transcript, and the waiting panel's Prompt cache section.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import type { CacheKeepAliveStatus } from '../../../generated/thread-event-wire';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import { threadMap } from '../../../store/store';
import { formatShortTime } from '../../../utils/formatTime';
import { CACHE_KEEP_ALIVE_EXPLAINER } from '../../../utils/engineEventExplainers';
import { KeepAliveRow, KeepAliveSection, nextBeatText, shownKeepAliveStatus } from '../CacheKeepAlive';
import { waitingPanelBody } from '../WaitingPanel';
import { describeInitiator } from '../ChatExchange';
import type { StoredEvent } from '../../../store/thread-events';
import { ENGINE_LABEL } from '../../../store/thread-events';

vi.mock('../../../store/actions/threads', () => ({ focusThreadOrBootstrap: vi.fn() }));

const NOW = Date.parse('2026-10-19T10:00:00Z');
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

function threadWith(status: CacheKeepAliveStatus | null, threadStatus: 'idle' | 'running' = 'idle') {
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

describe('the keep-alive card', () => {
  function mountCard(card: CacheKeepAliveStatus, thread = threadWith(card)) {
    threadMap.value = new Map([['t1', thread]]);
    return mount(<KeepAliveRow threadId="t1" status={card} />);
  }

  afterEach(() => {
    threadMap.value = new Map();
  });

  it('counts the pings, the cost and the time to the next ping while its run is open', () => {
    vi.useFakeTimers({ now: NOW });
    const root = mountCard(warming());
    expect(root.textContent).toContain('Cache kept warm');
    expect(root.textContent).toContain('Warming');
    expect(root.textContent).toContain('12 pings · $0.31 · next in 3m');
  });

  it('keeps its record after the run closes, without a countdown', () => {
    const closed = warming({ beats: 3 });
    const root = mountCard(closed, threadWith(null));
    expect(root.textContent).toContain('3 pings · $0.31');
    expect(root.textContent).not.toContain('next');
    expect(root.querySelector('[data-role="cache-keep-alive"]')?.getAttribute('data-state')).toBe('warming');
  });

  it('does not count down for an older wait of the same thread', () => {
    const older = warming({ since: '2026-10-19T08:00:00Z' });
    const root = mountCard(older, threadWith(warming()));
    expect(root.textContent).not.toContain('next');
  });

  it('does not count down while the thread runs', () => {
    const root = mountCard(warming(), threadWith(warming(), 'running'));
    expect(root.textContent).not.toContain('next');
  });

  it('says when the pings stopped', () => {
    const root = mountCard(stopped);
    expect(root.textContent).toContain('Stopped');
    expect(root.textContent).toContain(`25 pings · $0.64 · stopped at ${formatShortTime(new Date(GIVE_UP))}`);
  });

  it('draws nothing for a thread it does not keep warm', () => {
    const root = mountCard({ state: 'not_kept', reason: 'no_wake_children' });
    expect(root.querySelector('[data-role="cache-keep-alive"]')).toBeNull();
  });

  it('explains itself with the one shared explainer', () => {
    const root = mountCard(warming());
    expect(root.querySelector('.event-name')?.getAttribute('data-tooltip')).toBe(CACHE_KEEP_ALIVE_EXPLAINER);
  });

  it('draws nothing as the status of a running thread', () => {
    expect(shownKeepAliveStatus(threadWith(warming(), 'running'))).toBeNull();
  });

  it('counts seconds under a minute, and says when a ping is due', () => {
    expect(nextBeatText(40)).toBe('next in 40s');
    expect(nextBeatText(0)).toBe('next ping due');
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
  it('says how far into its budget the keep-alive is, and why', () => {
    const root = mount(<KeepAliveSection status={warming()} subThreadCount={1} />);
    expect(root.textContent).toContain('Kept warm: 12 of 25 pings, $0.31 so far');
    expect(root.textContent).toContain(
      `A ping every 4 minutes, so the sub-thread's report doesn't pay to reread 262k tokens. Stops around ${formatShortTime(new Date(GIVE_UP))}.`,
    );
    expect((root.querySelector('.waiting-panel-cache-meter > span') as HTMLElement).style.width).toBe('48%');
  });

  it('names the wake-up when no sub-thread will report', () => {
    const root = mount(<KeepAliveSection status={warming()} subThreadCount={0} />);
    expect(root.textContent).toContain("so the wake-up doesn't pay");
  });

  it('says it stopped at break-even', () => {
    const root = mount(<KeepAliveSection status={stopped} subThreadCount={1} />);
    expect(root.textContent).toContain(`Stopped keeping the cache warm at ${formatShortTime(new Date(GIVE_UP))}: past break-even.`);
    expect(root.textContent).toContain(
      "25 pings, $0.64. More pings would cost more than rereading 262k tokens once, so the sub-thread's report pays that reread instead.",
    );
    expect(root.querySelector('.waiting-panel-cache-meter')).toBeNull();
  });

  it('says why it keeps nothing warm', () => {
    const noWake = mount(<KeepAliveSection status={{ state: 'not_kept', reason: 'no_wake_children' }} subThreadCount={2} />);
    expect(noWake.textContent).toContain("Not keeping the cache warm: these sub-threads won't wake this thread.");
    const awaiting = mount(<KeepAliveSection status={{ state: 'not_kept', reason: 'children_awaiting_user' }} subThreadCount={2} />);
    expect(awaiting.textContent).toContain('Not keeping the cache warm: the sub-threads are waiting on you.');
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
