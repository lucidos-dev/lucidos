import { describe, it, expect } from 'vitest';
import { pickTurnIndex, pickTurnTarget } from '../scrollState';

const TOPS = [0, 500, 1200];
const T = 12;

describe('pickTurnIndex', () => {
  it('returns null for an empty transcript', () => {
    expect(pickTurnIndex([], 0, 1, T)).toBeNull();
    expect(pickTurnIndex([], 0, -1, T)).toBeNull();
  });

  it('next: from the very top lands on the second turn', () => {
    expect(pickTurnIndex(TOPS, 0, 1, T)).toBe(1);
  });

  it('next: after landing on a turn (scrollTop = top - gap) skips to the FOLLOWING turn', () => {
    expect(pickTurnIndex(TOPS, 492, 1, T)).toBe(2);
  });

  it('next: returns null at the last turn (nowhere below)', () => {
    expect(pickTurnIndex(TOPS, 1200, 1, T)).toBeNull();
    expect(pickTurnIndex(TOPS, 1192, 1, T)).toBeNull();
  });

  it('prev: from the bottom lands on the previous turn', () => {
    expect(pickTurnIndex(TOPS, 1200, -1, T)).toBe(1);
  });

  it('prev: after landing on a turn skips to the PRECEDING turn', () => {
    expect(pickTurnIndex(TOPS, 1192, -1, T)).toBe(1);
  });

  it('prev: returns null at the first turn (nowhere above)', () => {
    expect(pickTurnIndex(TOPS, 0, -1, T)).toBeNull();
    expect(pickTurnIndex(TOPS, 8, -1, T)).toBeNull();
  });

  it('prev from mid-turn snaps to the current turn top, then steps up on the next press', () => {
    expect(pickTurnIndex(TOPS, 600, -1, T)).toBe(1);
    expect(pickTurnIndex(TOPS, 492, -1, T)).toBe(0);
  });
});

describe('pickTurnIndex — landing-line calling convention (stepThreadTurn)', () => {
  const GAP = 56;
  const SLACK = 4;
  const SHORT_ADJACENT = [0, 500, 540];

  it('prev reaches a short turn immediately above the landed one', () => {
    expect(pickTurnIndex(SHORT_ADJACENT, 484 + GAP, -1, SLACK)).toBe(1);
  });

  it('next reaches a short turn immediately below the landed one', () => {
    expect(pickTurnIndex(SHORT_ADJACENT, 444 + GAP, 1, SLACK)).toBe(2);
  });

  it('a re-press does not re-select the just-landed turn (either direction)', () => {
    const ref = (500 - GAP) + GAP;
    expect(pickTurnIndex(SHORT_ADJACENT, ref, 1, SLACK)).toBe(2);
    expect(pickTurnIndex(SHORT_ADJACENT, ref, -1, SLACK)).toBe(0);
  });
});

describe('pickTurnTarget — marker-anchored stepping (reaches a clamped-bottom cluster)', () => {
  const T = 12;


  const CLUSTER = [0, 500, 540];
  const CLAMPED_REF = 60;

  it('scroll-based stepping alone cannot advance past the cluster (the bug)', () => {
    expect(pickTurnIndex(CLUSTER, CLAMPED_REF, 1, T)).toBe(1);
  });

  it('with a marker anchored on the collapsed turn, ⌘↓ reaches the change card', () => {
    expect(pickTurnTarget(1, CLUSTER, CLAMPED_REF, 1, T)).toBe(2);
  });

  it('with a marker anchored, ⌘↑ steps to the previous turn by index', () => {
    expect(pickTurnTarget(1, CLUSTER, CLAMPED_REF, -1, T)).toBe(0);
  });

  it('anchored stepping returns null at each end', () => {
    expect(pickTurnTarget(2, CLUSTER, CLAMPED_REF, 1, T)).toBeNull();
    expect(pickTurnTarget(0, CLUSTER, CLAMPED_REF, -1, T)).toBeNull();
  });

  it('with no anchor (-1), falls back to scroll-based pickTurnIndex', () => {
    expect(pickTurnTarget(-1, TOPS, 0, 1, T)).toBe(pickTurnIndex(TOPS, 0, 1, T));
    expect(pickTurnTarget(-1, TOPS, 600, -1, T)).toBe(pickTurnIndex(TOPS, 600, -1, T));
  });
});
