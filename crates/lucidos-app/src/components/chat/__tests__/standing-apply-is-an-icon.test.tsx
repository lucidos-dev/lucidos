// @vitest-environment jsdom
/**
 * The prompt row's *standing apply* is an ICON with an on and off state.
 *
 * It was a green `action-btn` pill reading "Apply as it settles", and on a
 * phone that took over half the row. No shorter label fixed it, so the label
 * went. The Changes panel keeps the text, where there is room for it, and
 * `components/changes/ChangesView.test.tsx` pins that half.
 *
 * This file pins the two things an icon-only toggle owes. It must still carry
 * the word, for a reader who cannot see the glyph, and it must say which
 * state it is in. Both come from the same
 * `TaggedAction` the other surface renders as text, so the wordings cannot
 * drift apart.
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

import { getStandingApplyControl } from '../WaitingBanner';
import {
  changes,
  standingApplyThreadIds,
  armingStandingApplyThreadIds,
  threadMap,
  focusedThreadId,
} from '../../../store/store';
import type { Change } from '../../../api/client';
import type { ThreadState } from '../../../store/thread-events';

const THREAD = 'thread-1';

function makeChange(): Change {
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
    thread_unsettled: true,
    thread_working: true,
  } as Change;
}

function makeThread(): ThreadState {
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

/** The control as the prompt row draws it right now.
 *
 *  Re-resolved rather than re-rendered from a held vnode, because the armed
 *  signal reaches this button by two routes. The button reads it for its own
 *  class and fill, and the selector reads it for the label. `PromptInput`
 *  re-resolves on every render for that reason. A test that re-rendered the
 *  old vnode would assert a wording the row never shows.
 */
function control(): HTMLButtonElement {
  render(getStandingApplyControl(), host);
  const btn = host.querySelector<HTMLButtonElement>('button[data-role="standing-apply"]');
  if (!btn) throw new Error('the prompt row draws no standing apply');
  return btn;
}

beforeEach(() => {
  changes.value = { status: 'loaded', data: [makeChange()] };
  standingApplyThreadIds.value = new Set();
  armingStandingApplyThreadIds.value = new Set();
  threadMap.value = new Map([[THREAD, makeThread()]]);
  focusedThreadId.value = THREAD;
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

describe('the prompt row draws the standing apply as an icon', () => {
  it('wears the row\'s icon-button classes and no action-btn pill', () => {
    const btn = control();
    expect(btn.className).toContain('icon-btn');
    expect(btn.className).toContain('header-icon');
    expect(btn.className).not.toContain('action-btn');
  });

  it('shows a glyph and no text', () => {
    const btn = control();
    expect(btn.querySelector('svg')).not.toBeNull();
    expect(btn.textContent).toBe('');
  });

  // useFitsInOneRow sums every [data-row-item]; a control missing the
  // attribute lets the row overflow instead of lifting its liftable slot.
  it('is measured by the row-overflow hook', () => {
    expect(control().hasAttribute('data-row-item')).toBe(true);
  });
});

describe('the icon says which state it is in', () => {
  it('starts unarmed, and says so', () => {
    expect(control().getAttribute('aria-pressed')).toBe('false');
  });

  it('flips to pressed when the armed signal turns on', () => {
    standingApplyThreadIds.value = new Set([THREAD]);
    expect(control().getAttribute('aria-pressed')).toBe('true');
  });

  // The fill is the visible half of the same flip: an outlined flag off, a
  // solid one armed, over one shape. Colour alone would carry it for nobody
  // who cannot separate the two accents.
  it('fills the glyph once armed, and outlines it otherwise', () => {
    expect(control().querySelector('svg')?.getAttribute('fill')).toBe('none');
    standingApplyThreadIds.value = new Set([THREAD]);
    expect(control().querySelector('svg')?.getAttribute('fill')).toBe('currentColor');
  });

  it('takes the row\'s active class once armed', () => {
    expect(control().className).not.toContain('active');
    standingApplyThreadIds.value = new Set([THREAD]);
    expect(control().className).toContain('active');
  });
});

describe('the icon keeps the word it stopped showing', () => {
  it('names the action for a reader in both states', () => {
    expect(control().getAttribute('aria-label')).toBe('Apply as it settles');
    standingApplyThreadIds.value = new Set([THREAD]);
    expect(control().getAttribute('aria-label')).toBe('✓ Applying as it settles');
  });

  it('carries a tooltip that changes with the state', () => {
    const off = control().getAttribute('data-tooltip');
    standingApplyThreadIds.value = new Set([THREAD]);
    const on = control().getAttribute('data-tooltip');
    expect(off).toBeTruthy();
    expect(on).toBeTruthy();
    expect(on).not.toBe(off);
  });

  // The phone is why the label went, so the tooltip has to reach a finger. The
  // host shell reveals on a long press only for elements that opt in, so a
  // `data-tooltip` alone leaves a mobile reader an unexplained flag.
  it('opts the tooltip into a touch long press', () => {
    expect(control().hasAttribute('data-tooltip-longpress')).toBe(true);
  });

  // ADR 0168: `.icon-btn:disabled` sets `pointer-events: none`, which takes the
  // tooltip above out of reach. A tap mid-request is dropped by the handler.
  it('never renders disabled, in either state', () => {
    expect(control().disabled).toBe(false);
    armingStandingApplyThreadIds.value = new Set([THREAD]);
    standingApplyThreadIds.value = new Set([THREAD]);
    expect(control().disabled).toBe(false);
  });
});
