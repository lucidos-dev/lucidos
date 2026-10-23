// @vitest-environment jsdom
/**
 * A change the landing lane holds says where it stands, on its Changes panel
 * row and on its thread's prompt row. In the open batch it reads "Landing in
 * a batch" and links the lane thread. Waiting, it reads "Waiting to land", and
 * the Changes panel row can take it back out.
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
const { removeSpy } = vi.hoisted(() => ({ removeSpy: vi.fn() }));
vi.mock('../../../store/actions/laneControls', () => ({ removeFromLandingLane: removeSpy }));

import { ChangesView } from '../ChangesView';
import { getBannerActions, getWaitingState } from '../../chat/WaitingBanner';
import { focusThread } from '../../../store/actions/threads';
import {
  appliedChanges,
  changes,
  EMPTY_LANDING_LANE,
  focusedThreadId,
  landingLane,
  setAsideChanges,
  threadMap,
} from '../../../store/store';
import { makeOptimisticThreadState } from '../../../store/thread-events';
import type { Change } from '../../../api/client';

function makeChange(id: string, threadId: string): Change {
  return {
    id,
    request_id: '00000000-0000-0000-0000-000000000000',
    thread_id: threadId,
    thread_title: `Thread ${id}`,
    branch_name: `b-${id}`,
    repo_root: '/r',
    description: `desc ${id}`,
    file_count: 1,
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
  };
}

let host: HTMLDivElement;

beforeEach(() => {
  changes.value = {
    status: 'loaded',
    data: [makeChange('c-batch', 't-batch'), makeChange('c-wait', 't-wait'), makeChange('c-free', 't-free')],
  };
  appliedChanges.value = { status: 'loaded', data: [] };
  setAsideChanges.value = { status: 'loaded', data: [] };
  landingLane.value = {
    open_batch: { batch_id: 'b-1', lane_thread_id: 'lane-t', members: ['c-batch'], step: 'resolving' },
    waiting: ['c-wait'],
    gathering: false,
  };
  // Each thread has finished its turn and proposed its change.
  threadMap.value = new Map(['t-batch', 't-wait', 't-free'].map((id) => {
    const thread = makeOptimisticThreadState({ id, title: `Thread ${id}`, channel: 'claude_code', initiator: 'user', eventsLoaded: true });
    return [id, {
      ...thread,
      meta: { ...thread.meta, status: 'idle', codingAgentChangeState: { kind: 'proposed', requires_restart: false } },
    }];
  }));
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  landingLane.value = EMPTY_LANDING_LANE;
  focusedThreadId.value = null;
  vi.clearAllMocks();
});

function row(changeId: string): HTMLElement {
  const found = [...host.querySelectorAll<HTMLElement>('.change-row')]
    .find((r) => r.textContent?.includes(`desc ${changeId}`));
  if (!found) throw new Error(`no row for ${changeId}`);
  return found;
}

function primary(r: HTMLElement): HTMLButtonElement {
  return r.querySelector<HTMLButtonElement>('.change-row-primary')!;
}

describe('the Changes panel row', () => {
  it('reads "Landing in a batch" in the open batch, and links its lane thread', () => {
    render(<ChangesView />, host);
    const r = row('c-batch');
    expect(r.querySelector('.list-row-details')?.textContent).toContain('Landing in a batch');
    const button = primary(r);
    expect(button.textContent).toBe('Lane thread');
    expect(button.disabled).toBe(false);
    button.click();
    expect(focusThread).toHaveBeenCalledWith('lane-t');
  });

  it('reads "Waiting to land" while it waits, and Remove takes it out', () => {
    render(<ChangesView />, host);
    const r = row('c-wait');
    expect(r.querySelector('.list-row-details')?.textContent).toContain('Waiting to land');
    const button = primary(r);
    expect(button.textContent).toBe('Remove');
    button.click();
    expect(removeSpy).toHaveBeenCalledWith('c-wait');
  });

  it('leaves a change outside the lane as it was', () => {
    render(<ChangesView />, host);
    const r = row('c-free');
    expect(r.querySelector('.list-row-details')?.textContent).not.toMatch(/Landing in a batch|Waiting to land/);
    expect(r.textContent).toContain('Apply');
  });
});

describe("the thread's prompt row", () => {
  it('in the open batch, says so and opens the lane thread', () => {
    focusedThreadId.value = 't-batch';
    const state = getWaitingState();
    expect(state).toEqual({ type: 'landing', place: 'in-batch', laneThreadId: 'lane-t' });
    const [member] = getBannerActions(state as Exclude<typeof state, null | { type: 'canceling' }>);
    expect(member.label).toBe('Landing in a batch');
    expect(member.disabledTooltip).toBeUndefined();
    member.onClick?.(new MouseEvent('click'));
    expect(focusThread).toHaveBeenCalledWith('lane-t');
  });

  it('waiting, shows the one disabled progress face', () => {
    focusedThreadId.value = 't-wait';
    const state = getWaitingState();
    expect(state).toMatchObject({ type: 'landing', place: 'waiting' });
    const [member] = getBannerActions(state as Exclude<typeof state, null | { type: 'canceling' }>);
    expect(member.label).toBe('Waiting to land');
    expect(member.disabledTooltip).toBe('Waiting to land');
  });

  it('outside the lane, offers Apply as before', () => {
    focusedThreadId.value = 't-free';
    expect(getWaitingState()?.type).not.toBe('landing');
  });
});
