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

/** Where a call's clock starts, for the first chunk that has no cursor yet. */
const OPENED_AT = 10;

/**
 * Run more chunks through, arriving at the pace a person hears them.
 *
 * A talker streaming audio in real time holds the queue at exactly the
 * cushion's depth and never deeper. That is the Live shape, and it is what
 * makes the cushion the whole jitter buffer.
 */
function atTalkingPace(state: Playback, chunks: number): Playback {
  let carried = state;
  for (let i = 0; i < chunks; i++) {
    const now = carried.cursor === 0 ? OPENED_AT : carried.cursor - carried.lead;
    carried = placeChunk(carried, now, CHUNK).playback;
  }
  return carried;
}

/** When the next chunk lands, given a stream stalled for `stall` seconds. */
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
    // Audio still queued is played, whatever is left of the cushion. Pushing
    // it out to a full lead would open the very hole the lead guards against.
    const worn = { ...PLAYBACK_START, cursor: 10.05, lead: 0.5 };

    const placed = placeChunk(worn, OPENED_AT, CHUNK);

    expect(placed.startAt).toBe(worn.cursor);
  });

  /** The defect the reported call was. The cushion was 80 ms, and the engine
   *  stalls for longer than that whenever it writes a row. */
  it('absorbs a stall shorter than the cushion', () => {
    const state = atTalkingPace(PLAYBACK_START, 5);

    const placed = placeChunk(state, stalledBy(state, 0.15), CHUNK);

    expect(placed.startAt).toBe(state.cursor);
    expect(placed.playback.gaps).toBe(0);
  });

  it('opens a hole on the same stall with the cushion this replaced', () => {
    // The old constant, so the case above is pinned as a fix rather than as
    // arithmetic that happens to pass.
    const thin = atTalkingPace({ ...PLAYBACK_START, lead: 0.08 }, 5);

    const placed = placeChunk(thin, stalledBy(thin, 0.15), CHUNK);

    expect(placed.startAt).toBeGreaterThan(thin.cursor);
    expect(placed.playback.gaps).toBe(1);
  });
});

describe('a queue that ran dry', () => {
  it('counts the hole, and the whole of the silence in it', () => {
    // The chunk was 0.3s late and then took a 0.5s lead. The caller sat
    // through both, so reporting the lateness alone understates the hole.
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
    // The whole point of growing. One hole is a hole. The same hole on every
    // reply for two minutes is what a caller calls crackling.
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
    // A reply that ended. The Realtime talker leaves one at every turn
    // boundary. Learning from those would cap the cushion on a clean call.
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
    // The caller cut the talker off, so the silence after it is theirs. Read
    // as a hole, every barge-in would grow the cushion for nothing.
    const cut = restartPlayback(atTalkingPace(PLAYBACK_START, 5));

    const placed = placeChunk(cut, 11, CHUNK);

    expect(placed.playback.gaps).toBe(0);
  });
});
