import { describe, it, expect } from 'vitest';
import {
  PLAYBACK_LEAD_MAX_SECONDS,
  PLAYBACK_LEAD_SECONDS,
  PLAYBACK_START,
  type Playback,
  placeChunk,
  restartPlayback,
} from './schedule';

const CHUNK = 0.04;

const OPENED_AT = 10;

function atTalkingPace(state: Playback, chunks: number): Playback {
  let carried = state;
  for (let i = 0; i < chunks; i++) {
    const now = carried.cursor === 0 ? OPENED_AT : carried.cursor - carried.lead;
    carried = placeChunk(carried, now, CHUNK).playback;
  }
  return carried;
}

function stalledBy(state: Playback, stall: number): number {
  return state.cursor - state.lead + stall;
}

describe('placing a chunk of talker audio', () => {
  it('starts a first chunk a lead ahead of now', () => {
    const placed = placeChunk(PLAYBACK_START, OPENED_AT, CHUNK);
    expect(placed.startAt).toBeCloseTo(OPENED_AT + PLAYBACK_LEAD_SECONDS, 10);
  });

  it('carries the cursor to the end of what it just placed', () => {
    const placed = placeChunk(PLAYBACK_START, OPENED_AT, CHUNK);
    expect(placed.playback.cursor).toBeCloseTo(placed.startAt + CHUNK, 10);
  });

  it('butts a following chunk against the one before it', () => {
    const first = placeChunk(PLAYBACK_START, OPENED_AT, CHUNK);
    const second = placeChunk(first.playback, 10.01, CHUNK);
    expect(second.startAt).toBe(first.playback.cursor);
  });

  it('leaves no gap across a run arriving at talking pace', () => {
    let state = PLAYBACK_START;
    let now = OPENED_AT;
    const starts: number[] = [];
    for (let i = 0; i < 20; i++) {
      const placed = placeChunk(state, now, CHUNK);
      starts.push(placed.startAt);
      state = placed.playback;
      now += CHUNK;
    }
    for (let i = 1; i < starts.length; i++) {
      expect(starts[i] - starts[i - 1]).toBeCloseTo(CHUNK, 10);
    }
    expect(state.gaps).toBe(0);
  });

  it('never schedules in the past when playback ran dry', () => {
    const placed = placeChunk({ ...PLAYBACK_START, cursor: 9.9 }, OPENED_AT, CHUNK);
    expect(placed.startAt).toBeGreaterThan(OPENED_AT);
  });

  it('holds a thinned queue together rather than re-leading it', () => {
    const worn = { ...PLAYBACK_START, cursor: 10.05, lead: 0.5 };

    const placed = placeChunk(worn, OPENED_AT, CHUNK);

    expect(placed.startAt).toBe(worn.cursor);
  });

  it('absorbs a stall shorter than the cushion', () => {
    const state = atTalkingPace(PLAYBACK_START, 5);

    const placed = placeChunk(state, stalledBy(state, 0.15), CHUNK);

    expect(placed.startAt).toBe(state.cursor);
    expect(placed.playback.gaps).toBe(0);
  });

  it('opens a hole on the same stall with the cushion this replaced', () => {
    const thin = atTalkingPace({ ...PLAYBACK_START, lead: 0.08 }, 5);

    const placed = placeChunk(thin, stalledBy(thin, 0.15), CHUNK);

    expect(placed.startAt).toBeGreaterThan(thin.cursor);
    expect(placed.playback.gaps).toBe(1);
  });
});

describe('a queue that ran dry', () => {
  it('counts the hole, and the whole of the silence in it', () => {
    const dry = { ...PLAYBACK_START, cursor: OPENED_AT };

    const placed = placeChunk(dry, 10.3, CHUNK);

    expect(placed.playback.gaps).toBe(1);
    expect(placed.playback.silentSeconds).toBeCloseTo(placed.startAt - OPENED_AT, 10);
    expect(placed.playback.silentSeconds).toBeCloseTo(0.8, 10);
  });

  it('grows the cushion to cover the hole it just heard', () => {
    const dry = { ...PLAYBACK_START, cursor: OPENED_AT };

    const placed = placeChunk(dry, 10.3, CHUNK);

    expect(placed.playback.lead).toBeCloseTo(0.3 + PLAYBACK_LEAD_SECONDS, 10);
  });

  it('absorbs the next stall of the same size, having grown for the first', () => {
    const grown = placeChunk({ ...PLAYBACK_START, cursor: OPENED_AT }, 10.3, CHUNK).playback;
    const settled = atTalkingPace(grown, 5);

    const placed = placeChunk(settled, stalledBy(settled, 0.3), CHUNK);

    expect(placed.startAt).toBe(settled.cursor);
    expect(placed.playback.gaps).toBe(grown.gaps);
  });

  it('never grows the cushion past the cap', () => {
    const dry = { ...PLAYBACK_START, cursor: OPENED_AT };

    const placed = placeChunk(dry, 10.9, CHUNK);

    expect(placed.playback.lead).toBe(PLAYBACK_LEAD_MAX_SECONDS);
  });

  it('never shrinks the cushion a smaller hole does not need', () => {
    const grown = { ...PLAYBACK_START, cursor: OPENED_AT, lead: 0.5 };

    const placed = placeChunk(grown, 10.05, CHUNK);

    expect(placed.playback.lead).toBe(0.5);
  });

  it('reads a long silence as the talker stopping, not as a hole', () => {
    const between = { ...PLAYBACK_START, cursor: OPENED_AT };

    const placed = placeChunk(between, 14, CHUNK);

    expect(placed.playback.gaps).toBe(0);
    expect(placed.playback.lead).toBe(PLAYBACK_LEAD_SECONDS);
  });

  it('reads the first chunk of a call as no hole at all', () => {
    const placed = placeChunk(PLAYBACK_START, OPENED_AT, CHUNK);
    expect(placed.playback.gaps).toBe(0);
  });
});

describe('starting playback over', () => {
  it('drops the cursor, because the queue it described is gone', () => {
    const state = atTalkingPace(PLAYBACK_START, 5);
    expect(restartPlayback(state).cursor).toBe(0);
  });

  it('keeps what the call learned, because a barge-in is not a bad line', () => {
    const grown = placeChunk({ ...PLAYBACK_START, cursor: OPENED_AT }, 10.3, CHUNK).playback;

    const after = restartPlayback(grown);

    expect(after.lead).toBe(grown.lead);
    expect(after.gaps).toBe(grown.gaps);
    expect(after.silentSeconds).toBe(grown.silentSeconds);
  });

  it('counts no hole for the chunk that follows a cut', () => {
    const cut = restartPlayback(atTalkingPace(PLAYBACK_START, 5));

    const placed = placeChunk(cut, 11, CHUNK);

    expect(placed.playback.gaps).toBe(0);
  });
});
