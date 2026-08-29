import { describe, it, expect } from 'vitest';
import { PLAYBACK_LEAD_SECONDS, scheduleChunk } from './schedule';

const CHUNK = 0.04;

describe('placing a chunk of talker audio', () => {
  it('starts a first chunk a lead ahead of now', () => {
    const placed = scheduleChunk(0, 10, CHUNK);
    expect(placed.startAt).toBeCloseTo(10 + PLAYBACK_LEAD_SECONDS, 10);
  });

  it('carries the cursor to the end of what it just placed', () => {
    const placed = scheduleChunk(0, 10, CHUNK);
    expect(placed.cursor).toBeCloseTo(placed.startAt + CHUNK, 10);
  });

  it('butts a following chunk against the one before it', () => {
    const first = scheduleChunk(0, 10, CHUNK);
    const second = scheduleChunk(first.cursor, 10.01, CHUNK);
    expect(second.startAt).toBe(first.cursor);
  });

  it('never schedules in the past when playback ran dry', () => {
    const placed = scheduleChunk(9, 10, CHUNK);
    expect(placed.startAt).toBeGreaterThan(10);
  });

  it('leaves no gap across a whole run of chunks', () => {
    let cursor = 0;
    let now = 10;
    const starts: number[] = [];
    for (let i = 0; i < 20; i++) {
      const placed = scheduleChunk(cursor, now, CHUNK);
      starts.push(placed.startAt);
      cursor = placed.cursor;
      now += CHUNK / 2; // chunks arrive faster than they play
    }
    for (let i = 1; i < starts.length; i++) {
      expect(starts[i] - starts[i - 1]).toBeCloseTo(CHUNK, 10);
    }
  });

  it('takes the lead from the caller', () => {
    expect(scheduleChunk(0, 10, CHUNK, 0).startAt).toBe(10);
  });
});
