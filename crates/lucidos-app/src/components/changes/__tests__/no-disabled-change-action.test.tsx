// @vitest-environment jsdom
/**
 * **Invariant: no change action renders disabled, on either surface.**
 *
 * ADR 0168. `.action-btn:disabled` and `.icon-btn:disabled` both set
 * `pointer-events: none`, so the tooltip explaining the block can never be
 * read. A control that cannot act is replaced by the one that can, which is
 * the standing apply.
 *
 * Rendered rather than asserted through the pure selectors. What is banned is a
 * `disabled` attribute in the markup, and a selector can be right while the JSX
 * beside it still draws one.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';

vi.mock('../../../store/actions/threads', () => ({
  focusThreadOrBootstrap: vi.fn(),
  focusThread: vi.fn(),
}));
vi.mock('../../../store/actions/repositories', () => ({
  viewChangeDiff: vi.fn(),
  viewThreadCcDiff: vi.fn(),
}));

// Only the workspace-scope disarm is replaced, so every other action keeps its
// real identity and the module's own constants still resolve.
const { disarmAll } = vi.hoisted(() => ({ disarmAll: vi.fn() }));
vi.mock('../../../store/actions/chat-changes', async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  disarmAllStandingApplies: disarmAll,
}));

import { ChangesView } from '../ChangesView';
import { getStandingApplyControl } from '../../chat/WaitingBanner';
import {
  changes,
  appliedChanges,
  applyingChangeIds,
  applyingNowThreadIds,
  applyAllInProgress,
  standingApplyThreadIds,
  armingStandingApplyThreadIds,
  workingThreadCount,
  threadMap,
  focusedThreadId,
} from '../../../store/store';
import type { Change } from '../../../api/client';
import type { ThreadState } from '../../../store/thread-events';

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
    status: 'pending',
    created_at: '2026-01-01T00:00:00Z',
    resolved_at: null,
    pre_merge_sha: null,
    post_merge_sha: null,
    commits: [],
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
      codingAgentProposed: true,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
      codingAgentHasDiff: true,
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
    data: [makeChange({ thread_unsettled: true, thread_working: true })],
  };
  appliedChanges.value = { status: 'loaded', data: [] };
  applyingChangeIds.value = new Set();
  applyingNowThreadIds.value = new Map();
  applyAllInProgress.value = false;
  standingApplyThreadIds.value = new Set();
  armingStandingApplyThreadIds.value = new Set();
  workingThreadCount.value = 1;
  disarmAll.mockClear();
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
    expect(actionLabels()).toContain('Apply as it settles');
    expect(actionLabels()).not.toContain('Apply');
    expect(actionLabels()).not.toContain('Discard');
  });

  it('shows the armed face, which cancels rather than re-arming', () => {
    standingApplyThreadIds.value = new Set([THREAD]);
    render(<ChangesView />, host);
    expect(actionLabels()).toContain('✓ Applying as it settles');
    expect(disabledActionButtons()).toEqual([]);
  });

  // The reason has to be readable somewhere. With no control left to hang a
  // tooltip on, the row says it in text.
  it('names the unsettled thread in the row details, not in a tooltip', () => {
    changes.value = {
      status: 'loaded',
      data: [makeChange({ thread_unsettled: true, thread_working: false })],
    };
    workingThreadCount.value = 0; // nothing to sweep, so no bulk row either
    render(<ChangesView />, host);
    expect(actionLabels()).toEqual(['Diff']);
    expect(host.textContent).toContain('The thread has not finished');
  });

  it('draws no disabled Apply for a change with nothing left in it', () => {
    changes.value = {
      status: 'loaded',
      data: [makeChange({ file_count: 0, thread_unsettled: false })],
    };
    render(<ChangesView />, host);
    expect(disabledActionButtons()).toEqual([]);
    // Discard IS how an emptied change is resolved, so it stays; Apply goes.
    expect(actionLabels()).toContain('Discard');
    expect(actionLabels()).not.toContain('Apply');
  });
});

/** The bulk control is the surface the bug was reported on: one green face that
 *  could only ever re-arm, with no off anywhere. It is a toggle now, wearing the
 *  shape the row and the prompt-row icon already wear. */
describe('the Changes panel bulk control', () => {
  // The toggle is the one control here carrying a pressed state. Naming it that
  // way cannot pick up Discard All or Apply All beside it.
  function bulkButton(): HTMLButtonElement {
    const btn = host.querySelector<HTMLButtonElement>('.changes-bulk-actions button[aria-pressed]');
    if (!btn) throw new Error('the bulk row draws no standing-apply toggle');
    return btn;
  }

  it('offers the arm while nothing is armed', () => {
    render(<ChangesView />, host);
    expect(bulkButton().textContent).toBe('Apply as they settle');
    expect(bulkButton().getAttribute('aria-pressed')).toBe('false');
  });

  it('shows the armed face and cancels on click, rather than re-arming', () => {
    standingApplyThreadIds.value = new Set([THREAD]);
    render(<ChangesView />, host);
    expect(bulkButton().textContent).toBe('✓ Applying as they settle');
    expect(bulkButton().getAttribute('aria-pressed')).toBe('true');
    bulkButton().click();
    expect(disarmAll).toHaveBeenCalledTimes(1);
  });

  // The one disabled face this control has is progress, not a blocked action.
  // A tooltip on it would be unreachable, which is the whole disabled ban.
  it('drops the tooltip on the in-flight face, which nobody could read', () => {
    applyAllInProgress.value = true;
    render(<ChangesView />, host);
    expect(bulkButton().textContent).toBe('Applying...');
    expect(bulkButton().disabled).toBe(true);
    expect(bulkButton().hasAttribute('data-tooltip')).toBe(false);
  });

  it('keeps the armed face live while a batch runs, so the off is reachable', () => {
    standingApplyThreadIds.value = new Set([THREAD]);
    applyAllInProgress.value = true;
    render(<ChangesView />, host);
    expect(bulkButton().disabled).toBe(false);
    expect(disabledActionButtons()).not.toContain('✓ Applying as they settle');
  });

  it('keeps the off drawn after the last thread stops working', () => {
    changes.value = { status: 'loaded', data: [] };
    workingThreadCount.value = 0;
    standingApplyThreadIds.value = new Set([THREAD]);
    render(<ChangesView />, host);
    expect(bulkButton().textContent).toBe('✓ Applying as they settle');
  });
});

/** The prompt row's control is an ICON, so it is not an `.action-btn` at all
 *  and the two helpers above cannot see it. The invariant is the same one:
 *  `.icon-btn:disabled` sets `pointer-events: none` just as its pill sibling
 *  does, so a disabled control here would take the same tooltip out of reach.
 *  What the icon itself draws is
 *  `components/chat/__tests__/standing-apply-is-an-icon.test.tsx`. */
describe("the thread's own prompt row", () => {
  function promptRowControl(): HTMLButtonElement {
    const btn = host.querySelector<HTMLButtonElement>('button[data-role="standing-apply"]');
    if (!btn) throw new Error('the prompt row draws no standing apply');
    return btn;
  }

  it('draws no disabled action, and offers the standing apply', () => {
    const control = getStandingApplyControl();
    expect(control, 'a working coding-agent thread must offer a change action').not.toBeNull();
    render(control, host);
    expect(promptRowControl().disabled).toBe(false);
    expect(promptRowControl().getAttribute('aria-label')).toBe('Apply as it settles');
  });

  it('carries a tooltip, which a disabled button would make unreachable', () => {
    render(getStandingApplyControl(), host);
    expect(promptRowControl().getAttribute('data-tooltip')).toBeTruthy();
  });

  it('offers nothing once the thread has settled, where Apply itself takes over', () => {
    threadMap.value = new Map([[THREAD, makeThread({ status: 'idle' })]]);
    expect(getStandingApplyControl()).toBeNull();
  });
});
