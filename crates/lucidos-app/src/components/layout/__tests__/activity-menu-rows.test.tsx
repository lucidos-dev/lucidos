// @vitest-environment jsdom
/**
 * The activity group in the Lucidos menu: one row per job in flight, each
 * unfolding its detail in place. A pending row stands for unbuilt code, and
 * an idle workspace draws nothing at all.
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { activityMenuGroup, ActivityMenuGroup, type ActivityGroupProps } from '../ActivityMenuRows';
import type { ActivityRow, ApplyThread } from '../../../store/actions/activityRows';
import { backgroundActivities } from '../../../store/backgroundActivity';
import {
  applyAllCanceling,
  applyAllInProgress,
  engineBuilding,
  engineRebuildWedged,
  engineVersionPending,
  toasts,
} from '../../../store/store';
import type { PendingCommits } from '../../../api/client';

const cancelApplyAllBatch = vi.fn(async () => {});
vi.mock('../../../store/actions/chat-changes', () => ({ cancelApplyAllBatch: () => cancelApplyAllBatch() }));

const COMMITS: PendingCommits = {
  total: 14,
  groups: [
    { kind: 'new', total: 1, descriptions: ['engine: store a coding agent\'s whole tool output'] },
    { kind: 'fixed', total: 3, descriptions: ['release: allow one placeholder'] },
    { kind: 'housekeeping', total: 10, descriptions: [] },
  ],
};

const BUILD = { elapsedMs: 19_000, anchoredAt: 1_000, pendingCommits: COMMITS };
const [engineBuild] = backgroundActivities(true, null, null, BUILD, 1_000);

const THREAD: ApplyThread = {
  threadId: 't-2',
  changeId: 'c-2',
  reading: { phase: 'hardening', eventId: 'e-9', startedAt: null },
  title: 'Collapse Menu',
  phase: 'Hardening · 10 min, usually ~20 min',
};

const buildRow: ActivityRow = {
  key: 'engine-build',
  label: 'Building new version',
  detail: '19s',
  body: { kind: 'background', activity: engineBuild },
};

const applyAllRow: ActivityRow = {
  key: 'apply-all',
  label: 'Applying 5 changes',
  detail: '2 of 5',
  body: {
    kind: 'apply-all',
    thread: THREAD,
    position: { index: 2, total: 5 },
    progress: { done: 0.2, working: 0.2 },
    timeLeft: 'about 28 min until all are applied',
    canceling: false,
  },
};

const noop = () => {};

function props(overrides: Partial<ActivityGroupProps>): ActivityGroupProps {
  return {
    rows: [],
    pending: 'none',
    open: new Set(),
    commits: null,
    onToggle: noop,
    onOpenThread: noop,
    onCancelApplyAll: noop,
    onOpenPending: noop,
    ...overrides,
  };
}

describe('activityMenuGroup', () => {
  let host: HTMLDivElement | null = null;

  function draw(p: ActivityGroupProps): HTMLDivElement {
    host = document.createElement('div');
    document.body.appendChild(host);
    render(activityMenuGroup(p), host);
    return host;
  }

  afterEach(() => {
    if (host) { render(null, host); host.remove(); host = null; }
  });

  it('renders nothing, separator included, when nothing runs', () => {
    expect(activityMenuGroup(props({}))).toBeNull();
  });

  it('draws each job on one row with its detail, folded', () => {
    const el = draw(props({ rows: [buildRow] }));
    const row = el.querySelector('.brand-menu-activity-row')!;
    expect(row.textContent).toContain('Building new version');
    expect(row.textContent).toContain('19s');
    expect(row.getAttribute('aria-expanded')).toBe('false');
    expect(el.querySelector('[data-role="activity-body"]')).toBeNull();
  });

  it('toggles a row by its key', () => {
    const onToggle = vi.fn();
    const el = draw(props({ rows: [buildRow], onToggle }));
    el.querySelector<HTMLButtonElement>('.brand-menu-activity-row')!.click();
    expect(onToggle).toHaveBeenCalledWith('engine-build');
  });

  it('lays out what an open build brings from the engine\'s groups', () => {
    const el = draw(props({ rows: [buildRow], open: new Set(['engine-build']), commits: COMMITS }));
    const changes = el.querySelector('[data-role="activity-changes"]')!;
    expect(changes.querySelector('.brand-menu-activity-headline')?.textContent)
      .toBe('14 commits come with the new version');
    const titles = [...changes.querySelectorAll('.brand-menu-activity-group-title')].map((n) => n.textContent);
    expect(titles).toEqual(['New', 'Fixed']);
    // Fixed shows one of its three, and says so rather than under-reporting.
    expect(changes.textContent).toContain('and 2 more');
    expect(changes.querySelector('.brand-menu-activity-housekeeping')?.textContent)
      .toBe('10 housekeeping commits (docs, tests, chores)');
  });

  it('says what follows a build whose commits git could not count', () => {
    const el = draw(props({ rows: [buildRow], open: new Set(['engine-build']), commits: null }));
    expect(el.querySelector('[data-role="activity-changes"]')).toBeNull();
    expect(el.querySelector('[data-role="activity-body"]')?.textContent)
      .toBe('You can switch to it once the build finishes.');
  });

  it('shows a download\'s progress and its memory caveat', () => {
    const [download] = backgroundActivities(false, {
      model_id: 'multilingual-e5-small',
      load_state: { kind: 'downloading', downloaded_bytes: 50, total_bytes: 100 },
    });
    const row: ActivityRow = { key: download.kind, label: download.label, body: { kind: 'background', activity: download } };
    const el = draw(props({ rows: [row], open: new Set([download.kind]) }));
    expect(el.querySelector<HTMLElement>('.progress-bar-fill')?.style.width).toBe('50%');
    expect(el.textContent).toContain('will not be searchable in memory');
  });

  it('keeps both of an Expose run\'s actions while it waits on the user', () => {
    const run = { phase: 'awaiting-tailnet-approval', url: 'https://login.tailscale.com/f/serve' } as const;
    const [serve] = backgroundActivities(false, null, run);
    const row: ActivityRow = { key: serve.kind, label: serve.label, body: { kind: 'background', activity: serve } };
    const el = draw(props({ rows: [row], open: new Set([serve.kind]) }));
    const labels = [...el.querySelectorAll('.brand-menu-activity-actions button')].map((b) => b.textContent);
    expect(labels).toEqual(['Cancel', 'Enable in Tailscale']);
  });

  it('opens an Apply All with its position, its thread and Cancel', () => {
    const onOpenThread = vi.fn();
    const onCancelApplyAll = vi.fn();
    const el = draw(props({ rows: [applyAllRow], open: new Set(['apply-all']), onOpenThread, onCancelApplyAll }));
    const body = el.querySelector('[data-role="activity-body"]')!;
    expect(body.textContent).toContain('Change 2 of 5 · about 28 min until all are applied');
    // The finished member fills, and the member in flight owns its own span.
    expect(body.querySelector<HTMLElement>('.progress-bar-fill')?.style.width).toBe('20%');
    const working = body.querySelector<HTMLElement>('.brand-menu-activity-working');
    expect([working?.style.left, working?.style.width]).toEqual(['20%', '20%']);
    const link = body.querySelector<HTMLButtonElement>('[data-role="activity-thread-link"]')!;
    expect(link.textContent).toContain('Hardening · 10 min, usually ~20 min');
    expect(link.textContent).toContain('Collapse Menu');
    link.click();
    expect(onOpenThread).toHaveBeenCalledWith(THREAD);
    [...body.querySelectorAll('button')].find((b) => b.textContent === 'Cancel')!.click();
    expect(onCancelApplyAll).toHaveBeenCalledOnce();
  });

  it('drops Cancel once it has been pressed', () => {
    const canceling: ActivityRow = {
      ...applyAllRow,
      label: 'Canceling apply...',
      body: { ...applyAllRow.body, canceling: true } as ActivityRow['body'],
    };
    const el = draw(props({ rows: [canceling], open: new Set(['apply-all']) }));
    expect([...el.querySelectorAll('button')].some((b) => b.textContent === 'Cancel')).toBe(false);
  });

  it('keeps Cancel before the batch names its member', () => {
    const bare: ActivityRow = {
      key: 'apply-all',
      label: 'Applying changes',
      body: { kind: 'apply-all', thread: null, position: null, progress: null, timeLeft: null, canceling: false },
    };
    const el = draw(props({ rows: [bare], open: new Set(['apply-all']) }));
    expect(el.querySelector('[data-role="activity-thread-link"]')).toBeNull();
    expect([...el.querySelectorAll('button')].some((b) => b.textContent === 'Cancel')).toBe(true);
  });

  it('opens a single apply to a link to its thread', () => {
    const row: ActivityRow = { key: 'apply-t-2', label: 'Hardening: Collapse Menu', body: { kind: 'apply-thread', thread: THREAD } };
    const el = draw(props({ rows: [row], open: new Set(['apply-t-2']) }));
    expect(el.querySelector('[data-role="activity-thread-link"]')?.textContent).toContain('Collapse Menu');
  });

  it('draws a waiting job still, and a running one spinning', () => {
    const queued: ActivityRow = { ...buildRow, label: 'New version queued', queued: true };
    const running: ActivityRow = { ...buildRow, key: 'frontend-refresh', label: 'Building frontend' };
    const el = draw(props({ rows: [queued, running] }));
    const marks = [...el.querySelectorAll('.brand-menu-activity-mark')];
    expect(marks.map((m) => [!!m.querySelector('.mini-spinner'), !!m.querySelector('svg')])).toEqual([
      [false, true],
      [true, false],
    ]);
  });

  it('says what holds the build slots once a queued build is opened', () => {
    const [waiting] = backgroundActivities(true, null, null, { ...BUILD, queuedBehind: ['make lint', 'engine tests'] }, 1_000);
    const row: ActivityRow = { ...buildRow, label: waiting.label, queued: true, body: { kind: 'background', activity: waiting } };
    const el = draw(props({ rows: [row], open: new Set(['engine-build']), commits: COMMITS }));
    expect(el.querySelector('.brand-menu-activity-label')?.textContent).toBe('New version queued');
    expect(el.querySelector('[data-role="activity-body"]')?.textContent).toContain('make lint, engine tests');
    expect(el.querySelector('[data-role="activity-changes"]')).not.toBeNull();
  });

  it('keeps a row for pending code, which is how its toast is found again', () => {
    const el = draw(props({ pending: 'wedged' }));
    expect(el.textContent).toContain('New code pending, no rebuild can deliver it');
  });
});

describe('ActivityMenuGroup', () => {
  let host: HTMLDivElement | null = null;

  function mount(onClose = vi.fn()) {
    host = document.createElement('div');
    document.body.appendChild(host);
    render(<ActivityMenuGroup onClose={onClose} />, host);
    return { host, onClose };
  }

  afterEach(() => {
    if (host) { render(null, host); host.remove(); host = null; }
    engineVersionPending.value = false;
    engineRebuildWedged.value = false;
    engineBuilding.value = false;
    applyAllInProgress.value = false;
    applyAllCanceling.value = false;
    toasts.value = [];
    vi.clearAllMocks();
  });

  it('unfolds a row in place and leaves the menu open', async () => {
    engineBuilding.value = true;
    const { host: el, onClose } = mount();
    await act(async () => { el.querySelector<HTMLButtonElement>('.brand-menu-activity-row')!.click(); });
    expect(el.querySelector('.brand-menu-activity-row')?.getAttribute('aria-expanded')).toBe('true');
    expect(el.querySelector('[data-role="activity-body"]')).not.toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    expect(toasts.value).toEqual([]);
  });

  it('cancels a running Apply All from its detail, without closing the menu', async () => {
    applyAllInProgress.value = true;
    const { host: el, onClose } = mount();
    await act(async () => { el.querySelector<HTMLButtonElement>('.brand-menu-activity-row')!.click(); });
    await act(async () => {
      [...el.querySelectorAll('button')].find((b) => b.textContent === 'Cancel')!.click();
    });
    expect(cancelApplyAllBatch).toHaveBeenCalledOnce();
    expect(onClose).not.toHaveBeenCalled();
  });

  it('closes the menu and re-opens the pending version toast from its row', () => {
    engineVersionPending.value = true;
    const { host: el, onClose } = mount();
    const pendingRow = [...el.querySelectorAll('button')].find((b) => b.textContent?.includes('New code pending'));
    pendingRow?.click();
    expect(onClose).toHaveBeenCalledOnce();
    expect(toasts.value.find((t) => t.key === 'engine-new-version')?.action?.label).toBe('Rebuild');
  });
});
