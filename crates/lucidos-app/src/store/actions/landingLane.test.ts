import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { LandingLane } from '../../api/client';
import { EMPTY_LANDING_LANE, landingLane, landingLaneCanceling, lanePlace } from '../store';
import { laneWithBatchEnded, laneWithBatchStarted, setLandingLaneFromServer } from './landingLane';

vi.mock('./threads', () => ({ focusThreadOrBootstrap: vi.fn() }));

const LANE: LandingLane = {
  open_batch: { batch_id: 'b-1', lane_thread_id: 'lane-t', members: ['c-1'], step: 'resolving' },
  waiting: ['c-2'],
  gathering: false,
};

describe('lanePlace', () => {
  it('tells a batch member from a waiting change and from one outside the lane', () => {
    expect(lanePlace(LANE, 'c-1')).toBe('in-batch');
    expect(lanePlace(LANE, 'c-2')).toBe('waiting');
    expect(lanePlace(LANE, 'c-3')).toBeNull();
    expect(lanePlace(LANE, undefined)).toBeNull();
    expect(lanePlace(EMPTY_LANDING_LANE, 'c-1')).toBeNull();
  });
});

describe('the lane frames, as pure steps', () => {
  it('a started batch takes its members off the waiting list', () => {
    const lane = laneWithBatchStarted({ open_batch: null, waiting: ['c-1', 'c-2'], gathering: false }, {
      batch_id: 'b-1', lane_thread_id: 'lane-t', members: [{ change_id: 'c-1' }],
    });
    expect(lane).toEqual(LANE);
  });

  it('only the open batch ends', () => {
    expect(laneWithBatchEnded(LANE, 'b-1')).toEqual({ open_batch: null, waiting: ['c-2'], gathering: false });
    expect(laneWithBatchEnded(LANE, 'b-2')).toBe(LANE);
  });
});

describe('setLandingLaneFromServer', () => {
  beforeEach(() => {
    landingLane.value = EMPTY_LANDING_LANE;
    landingLaneCanceling.value = null;
  });

  it('keeps a pressed Cancel while its batch is open, and drops it once it closed', () => {
    landingLaneCanceling.value = 'b-1';
    setLandingLaneFromServer(LANE);
    expect(landingLaneCanceling.value).toBe('b-1');
    setLandingLaneFromServer({ open_batch: null, waiting: [], gathering: false });
    expect(landingLaneCanceling.value).toBeNull();
  });

  it('reads an older engine with no lane as an empty lane', () => {
    landingLane.value = LANE;
    setLandingLaneFromServer(undefined);
    expect(landingLane.value).toEqual(EMPTY_LANDING_LANE);
  });
});
