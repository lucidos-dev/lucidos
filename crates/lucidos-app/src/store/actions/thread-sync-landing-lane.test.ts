/**
 * The landing lane's frames reach the `landingLane` signal by name, with no
 * translation layer: `LaneBatchStarted` opens the batch, `LaneBatchLanded`
 * and `LaneBatchFailed` close it, and `ChangesUpdated` carries the whole lane.
 * The main check's frames reach the `mainCheck` signal the same way.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';

const showToast = vi.fn();

vi.mock('../store', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../store')>()),
  showToast,
}));
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

const { handleGlobalEvent } = await import('./thread-sync');
const { EMPTY_LANDING_LANE, IDLE_MAIN_CHECK, landingLane, landingLaneCanceling, mainCheck } = await import('../store');

const STARTED = {
  batch_id: 'b-1',
  members: [{ change_id: 'c-1', snapshot_sha: 's1' }, { change_id: 'c-2', snapshot_sha: 's2' }],
  base_sha: 'base',
  lane_thread_id: 'lane-t',
};

describe('handleGlobalEvent: landing lane frames', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    landingLane.value = EMPTY_LANDING_LANE;
    landingLaneCanceling.value = null;
  });

  it('LaneBatchStarted opens the batch, resolving, and takes its members off the waiting list', () => {
    landingLane.value = { open_batch: null, waiting: ['c-1', 'c-3'], gathering: false };
    handleGlobalEvent('LaneBatchStarted', STARTED);
    expect(landingLane.value).toEqual({
      open_batch: { batch_id: 'b-1', lane_thread_id: 'lane-t', members: ['c-1', 'c-2'], step: 'resolving' },
      waiting: ['c-3'],
      gathering: false,
    });
  });

  it('LaneBatchLanded closes that batch and keeps what waits', () => {
    handleGlobalEvent('LaneBatchStarted', STARTED);
    landingLane.value = { ...landingLane.value, waiting: ['c-3'] };
    handleGlobalEvent('LaneBatchLanded', { batch_id: 'b-1', pre_sha: 'a', post_sha: 'b' });
    expect(landingLane.value).toEqual({ open_batch: null, waiting: ['c-3'], gathering: false });
    expect(showToast).not.toHaveBeenCalled();
  });

  it('a frame for another batch changes nothing', () => {
    handleGlobalEvent('LaneBatchStarted', STARTED);
    const before = landingLane.value;
    handleGlobalEvent('LaneBatchFailed', { batch_id: 'b-other', reason: 'x' });
    expect(landingLane.value).toBe(before);
  });

  it('a failure nobody asked for says why, and a Cancel says nothing', () => {
    handleGlobalEvent('LaneBatchStarted', STARTED);
    handleGlobalEvent('LaneBatchFailed', { batch_id: 'b-1', reason: 'The lane thread was stopped.' });
    expect(landingLane.value.open_batch).toBeNull();
    expect(showToast).toHaveBeenCalledWith(
      'The lane thread was stopped. Its changes are still pending.',
      'error',
      expect.objectContaining({ title: 'Lane batch failed' }),
    );

    showToast.mockClear();
    handleGlobalEvent('LaneBatchStarted', STARTED);
    landingLaneCanceling.value = 'b-1';
    handleGlobalEvent('LaneBatchFailed', {
      batch_id: 'b-1',
      reason: 'Canceled from the landing lane.',
      actor: { kind: 'device', device_id: 'd-1' },
    });
    expect(showToast).not.toHaveBeenCalled();
    expect(landingLaneCanceling.value).toBeNull();
  });

  it('ChangesUpdated replaces the lane whole', () => {
    const lane = {
      open_batch: { batch_id: 'b-2', lane_thread_id: 'lane-2', members: ['c-4'], step: 'landing' as const },
      waiting: ['c-5'],
    };
    handleGlobalEvent('ChangesUpdated', { pending: [], applied: [], set_aside: [], landing_lane: lane });
    expect(landingLane.value).toEqual(lane);
    handleGlobalEvent('ChangesUpdated', { pending: [], applied: [], set_aside: [] });
    expect(landingLane.value).toEqual(EMPTY_LANDING_LANE);
  });
});

describe('handleGlobalEvent: main check frames', () => {
  beforeEach(() => {
    mainCheck.value = IDLE_MAIN_CHECK;
  });

  it('reach the mainCheck signal by name, start to red to closed', () => {
    handleGlobalEvent('MainCheckStarted', { check_id: 'k-1', from_sha: 'aaa', to_sha: 'bbb' });
    expect(mainCheck.value).toMatchObject({ state: 'running', check_id: 'k-1' });
    handleGlobalEvent('MainCheckFailed', { check_id: 'k-1', summary: 'Failed: engine.', fix_thread_id: 'fix-t' });
    expect(mainCheck.value).toMatchObject({ state: 'red', fix_thread_id: 'fix-t', summary: 'Failed: engine.' });
    handleGlobalEvent('MainCheckRedClosed', { check_id: 'k-1', reason: 'closed' });
    expect(mainCheck.value).toEqual(IDLE_MAIN_CHECK);
  });

  it('ChangesUpdated replaces the main check whole', () => {
    const check = { state: 'passed' as const, check_id: 'k-2', to_sha: 'ccc' };
    handleGlobalEvent('ChangesUpdated', { pending: [], applied: [], set_aside: [], main_check: check });
    expect(mainCheck.value).toEqual(check);
    handleGlobalEvent('ChangesUpdated', { pending: [], applied: [], set_aside: [] });
    expect(mainCheck.value).toEqual(IDLE_MAIN_CHECK);
  });
});
