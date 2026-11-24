// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, type ComponentChild } from 'preact';

vi.mock('../../../store/actions/threads', () => ({
  focusThreadOrBootstrap: vi.fn(),
  focusThread: vi.fn(),
}));
vi.mock('../../../store/actions/repositories', () => ({
  viewChangeDiff: vi.fn(),
  viewThreadCcDiff: vi.fn(),
}));

const { armSection, disarmSection } = vi.hoisted(() => ({ armSection: vi.fn(), disarmSection: vi.fn() }));
vi.mock('../../../store/actions/chat-changes', async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  armStandingApplies: armSection,
  disarmStandingApplies: disarmSection,
}));

import { ChangesView, SETTLE_ALL_TIP } from '../ChangesView';
import { getStandaloneActions } from '../../chat/WaitingBanner';
import {
  changes,
  appliedChanges,
  setAsideChanges,
  applyingChangeIds,
  applyNowRequests,
  applyAllInProgress,
  standingApplyThreadIds,
  armingStandingApplyThreadIds,
  threadMap,
  focusedThreadId,
} from '../../../store/store';
import type { Change } from '../../../api/client';
import type { ThreadState } from '../../../store/thread-events';

function standingApply(): ComponentChild | null {
  const member = getStandaloneActions().find((m) => m.key === 'standing-apply');
  return member?.render ? member.render({}) : null;
}

const THREAD = 'thread-1';

function makeChange(over: Partial<Change> = {}): Change {
  return {
    id: 'change-1',
    request_id: '00000000-0000-0000-0000-000000000000',
    thread_id: THREAD,
    thread_title: 'Working thread',
    branch_name: 'b',
    repo_root: '/r',
    description: 'desc',
    file_count: 3,
    files: ['a.rs'],
    requires_restart: false,
    hardened: true,
    needs_hardening: false,
    apply_ready: true,
    status: 'pending',
    created_at: '2026-01-01T00:00:00Z',
    resolved_at: null,
    pre_merge_sha: null,
    post_merge_sha: null,
    commits: [],
    summary: null,
    incomplete: false,
    ...over,
  };
}

function makeThread(over: Partial<ThreadState['meta']> = {}): ThreadState {
  return {
    meta: {
      id: THREAD,
      title: 'Working thread',
      channel: 'claude_code',
      initiator: 'user',
      saved: false,
      createdAt: '',
      updatedAt: '',
      status: 'running',
      messageCount: 0,
      section: 'inbox',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0,
      attentionDescendantCount: 0,
      codingAgentChangeState: { kind: 'proposed', requires_restart: false },
      codingAgentIsExternalRepo: false,
      lastRevivedAt: '',
      state: 'active',
      latestTodoList: null,
      liveEventWaitCount: 0,
      liveEventWaits: [],
      ...over,
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  } as ThreadState;
}

let host: HTMLDivElement;

beforeEach(() => {
  changes.value = {
    status: 'loaded',
    data: [makeChange({ thread_unsettled: true, apply_ready: false, thread_settling: true })],
  };
  appliedChanges.value = { status: 'loaded', data: [] };
  setAsideChanges.value = { status: 'loaded', data: [] };
  applyingChangeIds.value = new Set();
  applyNowRequests.value = new Map();
  applyAllInProgress.value = false;
  standingApplyThreadIds.value = new Set();
  armingStandingApplyThreadIds.value = new Set();
  armSection.mockClear();
  disarmSection.mockClear();
  threadMap.value = new Map([[THREAD, makeThread()]]);
  focusedThreadId.value = THREAD;
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

function disabledActionButtons(): string[] {
  return [...host.querySelectorAll('button.action-btn')]
    .filter((b) => (b as HTMLButtonElement).disabled)
    .map((b) => b.textContent ?? '');
}

function actionLabels(): string[] {
  return [...host.querySelectorAll('button.action-btn')].map((b) => b.textContent ?? '');
}

describe('the Changes panel row', () => {
  it('draws no disabled action for a change whose thread is still working', () => {
    render(<ChangesView />, host);
    expect(disabledActionButtons()).toEqual([]);
  });

  it('offers the standing apply in place of the Apply it withholds', () => {
    render(<ChangesView />, host);
    expect(actionLabels()).toContain('Apply on settle');
    expect(actionLabels()).not.toContain('Apply');
    expect(actionLabels()).not.toContain('Discard');
  });

  it('shows the armed face, which cancels rather than re-arming', () => {
    standingApplyThreadIds.value = new Set([THREAD]);
    render(<ChangesView />, host);
    expect(actionLabels()).toContain('✓ Applying on settle');
    expect(disabledActionButtons()).toEqual([]);
  });

  it('names the unsettled thread in the row details, not in a tooltip', () => {
    changes.value = {
      status: 'loaded',
      data: [makeChange({ thread_unsettled: true, apply_ready: false, thread_settling: false })],
    };
    render(<ChangesView />, host);
    expect(actionLabels()).toEqual(['Diff']);
    expect(host.textContent).toContain('The thread has not finished');
  });

  it('shows an apply resolving merge conflicts as in flight, with no standing apply', () => {
    changes.value = {
      status: 'loaded',
      data: [makeChange({ thread_unsettled: true, apply_ready: false, thread_settling: true, resolving_conflict: true })],
    };
    render(<ChangesView />, host);
    expect(actionLabels()).not.toContain('Apply on settle');
    expect(actionLabels()).toContain('Resolving...');
    expect(host.textContent).toContain('Resolving merge conflicts');
    expect(host.textContent).not.toContain('The thread has not finished');
  });

  it('draws no disabled Apply for a change with nothing left in it', () => {
    changes.value = {
      status: 'loaded',
      data: [makeChange({ file_count: 0, thread_unsettled: false })],
    };
    render(<ChangesView />, host);
    expect(disabledActionButtons()).toEqual([]);
    expect(actionLabels()).toContain('Discard');
    expect(actionLabels()).not.toContain('Apply');
  });
});

describe('the Not finished bulk control', () => {
  function bulkButton(): HTMLButtonElement {
    const btn = host.querySelector<HTMLButtonElement>('.changes-bulk-actions button[aria-pressed]');
    if (!btn) throw new Error('the bulk row draws no standing-apply toggle');
    return btn;
  }

  it('offers the arm while nothing is armed, and arms the listed change', () => {
    render(<ChangesView />, host);
    expect(bulkButton().textContent).toBe('Apply all on settle');
    expect(bulkButton().getAttribute('aria-pressed')).toBe('false');
    bulkButton().click();
    expect(armSection.mock.calls[0][0].map((c: Change) => c.thread_id)).toEqual([THREAD]);
  });

  it('shows the armed face and cancels only its own arms on click', () => {
    standingApplyThreadIds.value = new Set([THREAD, 'elsewhere']);
    render(<ChangesView />, host);
    expect(bulkButton().textContent).toBe('✓ Applying all on settle');
    expect(bulkButton().getAttribute('aria-pressed')).toBe('true');
    bulkButton().click();
    expect(disarmSection).toHaveBeenCalledWith([THREAD]);
  });

  it('keeps its face, its tooltip and its press while an Apply All runs', () => {
    applyAllInProgress.value = true;
    render(<ChangesView />, host);
    expect(bulkButton().textContent).toBe('Apply all on settle');
    expect(bulkButton().disabled).toBe(false);
    expect(bulkButton().getAttribute('data-tooltip')).toBe(SETTLE_ALL_TIP);
  });

  it('keeps the armed face live while a batch runs, so the off is reachable', () => {
    standingApplyThreadIds.value = new Set([THREAD]);
    applyAllInProgress.value = true;
    render(<ChangesView />, host);
    expect(bulkButton().disabled).toBe(false);
    expect(disabledActionButtons()).not.toContain('✓ Applying all on settle');
  });

  it('draws no toggle when nothing in the section can be armed', () => {
    changes.value = {
      status: 'loaded',
      data: [makeChange({ thread_unsettled: true, apply_ready: false, thread_settling: false })],
    };
    render(<ChangesView />, host);
    expect(host.querySelector('.changes-bulk-actions')).toBeNull();
  });
});

describe("the thread's own prompt row", () => {
  function promptRowControl(): HTMLButtonElement {
    const btn = host.querySelector<HTMLButtonElement>('button[data-role="standing-apply"]');
    if (!btn) throw new Error('the prompt row draws no standing apply');
    return btn;
  }

  it('draws no disabled action, and offers the standing apply', () => {
    const control = standingApply();
    expect(control, 'a working coding-agent thread must offer a change action').not.toBeNull();
    render(control, host);
    expect(promptRowControl().disabled).toBe(false);
    expect(promptRowControl().getAttribute('aria-label')).toBe('Apply on settle');
  });

  it('carries a tooltip, which a disabled button would make unreachable', () => {
    render(standingApply(), host);
    expect(promptRowControl().getAttribute('data-tooltip')).toBeTruthy();
  });

  it('offers nothing while the thread resolves an apply that hit merge conflicts', () => {
    changes.value = {
      status: 'loaded',
      data: [makeChange({ thread_unsettled: true, apply_ready: false, thread_settling: true, resolving_conflict: true })],
    };
    expect(standingApply()).toBeNull();
  });

  it('offers nothing once the thread has settled, where Apply itself takes over', () => {
    threadMap.value = new Map([[THREAD, makeThread({ status: 'idle' })]]);
    expect(standingApply()).toBeNull();
  });
});
