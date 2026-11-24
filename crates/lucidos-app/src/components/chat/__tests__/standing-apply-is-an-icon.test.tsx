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

import { getStandaloneActions } from '../WaitingBanner';
import {
  changes,
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
    thread_unsettled: true,
    thread_settling: true,
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
      summaryVersion: 0,
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

function control(): HTMLButtonElement {
  render(standingApply(), host);
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

  it('takes the row marker from the composer rather than hardcoding it', () => {
    expect(control().hasAttribute('data-row-item')).toBe(false);
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

  it('fills the glyph once armed, and outlines it otherwise', () => {
    expect(control().querySelector('svg')?.getAttribute('fill')).toBe('none');
    standingApplyThreadIds.value = new Set([THREAD]);
    expect(control().querySelector('svg')?.getAttribute('fill')).toBe('currentColor');
  });

  it('wears the armed tone, pressed or not', () => {
    expect(control().getAttribute('data-tone')).toBe('armed');
    standingApplyThreadIds.value = new Set([THREAD]);
    expect(control().getAttribute('data-tone')).toBe('armed');
  });
});

describe('the prompt row draws no flag where Lucidos never applies', () => {
  function markExternal(meta: Partial<ThreadState['meta']>): void {
    const thread = makeThread();
    threadMap.value = new Map([[THREAD, { ...thread, meta: { ...thread.meta, ...meta } }]]);
  }

  it('draws nothing for a thread on an external repo', () => {
    markExternal({ codingAgentKind: 'external' });
    expect(standingApply()).toBeNull();
  });

  it('draws nothing for a legacy external-repo row', () => {
    markExternal({ codingAgentIsExternalRepo: true });
    expect(standingApply()).toBeNull();
  });

  it('draws nothing before the thread has proposed anything', () => {
    markExternal({ codingAgentKind: 'external', codingAgentChangeState: { kind: 'none' }});
    expect(standingApply()).toBeNull();
  });
});

describe('the icon keeps the word it stopped showing', () => {
  it('names the action for a reader in both states', () => {
    expect(control().getAttribute('aria-label')).toBe('Apply on settle');
    standingApplyThreadIds.value = new Set([THREAD]);
    expect(control().getAttribute('aria-label')).toBe('Applying on settle');
  });

  it('folds into the ⋯ menu as a toggle that says whether it is armed', () => {
    const member = () => getStandaloneActions().find((m) => m.key === 'standing-apply');
    expect(member()?.active).toBe(false);
    standingApplyThreadIds.value = new Set([THREAD]);
    expect(member()?.active).toBe(true);
  });

  it('carries a tooltip that changes with the state', () => {
    const off = control().getAttribute('data-tooltip');
    standingApplyThreadIds.value = new Set([THREAD]);
    const on = control().getAttribute('data-tooltip');
    expect(off).toBeTruthy();
    expect(on).toBeTruthy();
    expect(on).not.toBe(off);
  });

  it('opts the tooltip into a touch long press', () => {
    expect(control().hasAttribute('data-tooltip-longpress')).toBe(true);
  });

  it('never renders disabled, in either state', () => {
    expect(control().disabled).toBe(false);
    armingStandingApplyThreadIds.value = new Set([THREAD]);
    standingApplyThreadIds.value = new Set([THREAD]);
    expect(control().disabled).toBe(false);
  });
});
