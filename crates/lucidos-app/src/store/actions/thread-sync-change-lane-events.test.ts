/**
 * A change's own thread events move the lane and the pending list at once.
 * The `ChangesUpdated` snapshot trails them by a merge prediction per change,
 * and a row would read Apply until it arrived. The snapshot still replaces
 * both whole (ADR 0463).
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('../../api/client', () => ({ API_BASE: '', API: '/api/v1', postMcpConsent: vi.fn() }));
vi.mock('./notifications', () => ({ handleNotificationSSE: vi.fn() }));
vi.mock('./chat-changes', () => ({ addRestartGroup: vi.fn() }));
vi.mock('./preferences', () => ({ loadPreferences: vi.fn() }));
vi.mock('./threads', () => ({ focusThread: vi.fn() }));
vi.mock('./repositories', () => ({ refreshRepoView: vi.fn(), openEncodedRepoFilePreview: vi.fn(() => false) }));
vi.mock('./entityReferences', () => ({
  processSSEForReferences: vi.fn(),
  refreshLlmConfigured: vi.fn(),
  PROVIDER_PREFERENCE_KEYS: new Set(),
}));
vi.mock('./thread-loading', () => ({
  loadAllThreads: vi.fn(async () => {}),
  refreshThreadEvents: vi.fn(async () => {}),
}));

const { handleThreadEvent, handleGlobalEvent } = await import('./thread-sync');
const { EMPTY_LANDING_LANE, changes, landingLane, lanePlace, threadMap } = await import('../store');
import type { Change } from '../../api/client';
import type { ThreadState } from '../thread-events';

function thread(id: string): ThreadState {
  return {
    meta: { id, title: '', channel: 'chat', state: 'active', status: 'idle' },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  } as unknown as ThreadState;
}

let seq = 0;
function send(event: Record<string, unknown>): void {
  handleThreadEvent({ thread_id: 't-1', seq: ++seq, created: '2026-10-20T10:00:00Z', aggregate: {}, event });
}

describe('a change event moves the lane and the pending list', () => {
  beforeEach(() => {
    threadMap.value = new Map([['t-1', thread('t-1')]]);
    landingLane.value = EMPTY_LANDING_LANE;
    changes.value = {
      status: 'loaded',
      data: [{ id: 'c-1', thread_id: 't-1' }, { id: 'c-2', thread_id: 't-2' }] as Change[],
    };
  });

  it('ChangeQueued places the change as waiting, with no wait for ChangesUpdated', () => {
    expect(lanePlace(landingLane.value, 'c-1')).toBeNull();
    send({ type: 'ChangeQueued', change_id: 'c-1', snapshot_sha: 's1' });
    expect(lanePlace(landingLane.value, 'c-1')).toBe('waiting');
  });

  it('ChangeQueued leaves a change the lane already places alone', () => {
    const lane = {
      open_batch: { batch_id: 'b-1', lane_thread_id: 'lane-t', members: ['c-1'], step: 'resolving' as const },
      waiting: [],
      gathering: false,
    };
    landingLane.value = lane;
    send({ type: 'ChangeQueued', change_id: 'c-1', snapshot_sha: 's1' });
    expect(landingLane.value).toBe(lane);
  });

  it('ChangeUnqueued takes the change off the waiting list', () => {
    send({ type: 'ChangeQueued', change_id: 'c-1', snapshot_sha: 's1' });
    send({ type: 'ChangeUnqueued', change_id: 'c-1', reason: 'removed' });
    expect(lanePlace(landingLane.value, 'c-1')).toBeNull();
  });

  it('ChangeApplied drops the change from the pending list and the lane', () => {
    send({ type: 'ChangeQueued', change_id: 'c-1', snapshot_sha: 's1' });
    send({ type: 'ChangeApplied', change_id: 'c-1' });
    expect(lanePlace(landingLane.value, 'c-1')).toBeNull();
    expect(changes.value).toEqual({ status: 'loaded', data: [{ id: 'c-2', thread_id: 't-2' }] });
  });

  it('the next ChangesUpdated replaces the lane whole', () => {
    send({ type: 'ChangeQueued', change_id: 'c-1', snapshot_sha: 's1' });
    handleGlobalEvent('ChangesUpdated', { pending: [], applied: [], set_aside: [] });
    expect(landingLane.value).toEqual(EMPTY_LANDING_LANE);
  });
});
