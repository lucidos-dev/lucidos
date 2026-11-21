import { describe, it, expect } from 'vitest';
import { pickTurnIndex, pickTurnTarget } from '../scrollState';

// Three turns at content-space tops 0 / 500 / 1200. The threshold (12) is > the
// land gap (~8px), so a re-press right after landing (scrollTop == top - gap)
// never re-selects the turn it just landed on.
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
    // Landed on turn 1 (top 500) at scrollTop 492 — next must pick turn 2, not re-pick 1.
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
    // Landed on turn 2 (top 1200) at scrollTop 1192 — prev must pick turn 1.
    expect(pickTurnIndex(TOPS, 1192, -1, T)).toBe(1);
  });

  it('prev: returns null at the first turn (nowhere above)', () => {
    expect(pickTurnIndex(TOPS, 0, -1, T)).toBeNull();
    expect(pickTurnIndex(TOPS, 8, -1, T)).toBeNull();
  });

  it('prev from mid-turn snaps to the current turn top, then steps up on the next press', () => {
    // Reading inside turn 1 (which starts at 500) at scrollTop 600.
    expect(pickTurnIndex(TOPS, 600, -1, T)).toBe(1); // snap to turn 1's top (500)
    expect(pickTurnIndex(TOPS, 492, -1, T)).toBe(0); // a second prev → turn 0
  });
});

describe('pickTurnIndex — landing-line calling convention (stepThreadTurn)', () => {
  // stepThreadTurn lands a turn's top `gap` px below the container top (matching the
  // deep-link clearance, .chat-exchange scroll-margin-top ≈ 56px on desktop), then
  // calls pickTurnIndex with `scrollTop + gap` as the reference and only a small
  // slack as the threshold. That reference IS the landing line: `tops[i] > ref`
  // means "turn i's landing scroll position is forward of the current one". These
  // cases guard the regression where folding the gap into the THRESHOLD instead
  // (~2×gap skip band) made short adjacent turns unreachable by stepping.
  const GAP = 56;   // desktop clearance
  const SLACK = 4;  // TURN_NAV_THRESHOLD_SLACK_PX
  // A short turn (40px tall) sits between two taller ones: tops 0, 500, 540.
  const SHORT_ADJACENT = [0, 500, 540];

  it('prev reaches a short turn immediately above the landed one', () => {
    // Landed on turn 2 (top 540) → scrollTop = 540 - GAP = 484; reference = 540.
    // The short turn 1 (top 500) is only 40px above — well under 2×GAP — yet must
    // still be the prev target, not skipped to turn 0.
    expect(pickTurnIndex(SHORT_ADJACENT, 484 + GAP, -1, SLACK)).toBe(1);
  });

  it('next reaches a short turn immediately below the landed one', () => {
    // Landed on turn 1 (top 500) → scrollTop = 500 - GAP = 444; reference = 500.
    // The short turn 2 (top 540) is the next target.
    expect(pickTurnIndex(SHORT_ADJACENT, 444 + GAP, 1, SLACK)).toBe(2);
  });

  it('a re-press does not re-select the just-landed turn (either direction)', () => {
    // Landed on turn 1 (top 500) → reference = 500. next → turn 2 (not 1); prev →
    // turn 0 (not 1). The small slack absorbs the landed turn sitting on the line.
    const ref = (500 - GAP) + GAP;
    expect(pickTurnIndex(SHORT_ADJACENT, ref, 1, SLACK)).toBe(2);
    expect(pickTurnIndex(SHORT_ADJACENT, ref, -1, SLACK)).toBe(0);
  });
});

describe('pickTurnTarget — marker-anchored stepping (reaches a clamped-bottom cluster)', () => {
  const T = 12;

  // Regression: after collapsing the last turn, the collapsed turn + an appended
  // "Change applied" card cluster in the last (clamped-scroll) viewport. Pure
  // scroll-position stepping keys off scrollTop, which is pinned at the bottom, so
  // ⌘↓ keeps re-selecting the same turn (or returns null) and the change card is
  // unreachable. With a marker on a listed turn, stepping goes by INDEX instead.

  // A short thread that fits the viewport → maxScroll 0, scrollTop pinned at 0.
  // Reference = scrollTop + gap ≈ 60, so scroll-based `next` keeps returning turn 1.
  const CLUSTER = [0, 500, 540]; // turn2 = the appended change-applied card
  const CLAMPED_REF = 60;

  it('scroll-based stepping alone cannot advance past the cluster (the bug)', () => {
    // Both proofs of the bug: from the pinned reference, `next` re-selects turn 1
    // (never 2), and once "on" turn 1 there is nowhere for `prev` to go either.
    expect(pickTurnIndex(CLUSTER, CLAMPED_REF, 1, T)).toBe(1);
  });

  it('with a marker anchored on the collapsed turn, ⌘↓ reaches the change card', () => {
    // Anchor on turn 1 (the collapsed last turn) → next is turn 2 (the change card),
    // regardless of the pinned scroll position.
    expect(pickTurnTarget(1, CLUSTER, CLAMPED_REF, 1, T)).toBe(2);
  });

  it('with a marker anchored, ⌘↑ steps to the previous turn by index', () => {
    expect(pickTurnTarget(1, CLUSTER, CLAMPED_REF, -1, T)).toBe(0);
  });

  it('anchored stepping returns null at each end', () => {
    expect(pickTurnTarget(2, CLUSTER, CLAMPED_REF, 1, T)).toBeNull(); // last → no next
    expect(pickTurnTarget(0, CLUSTER, CLAMPED_REF, -1, T)).toBeNull(); // first → no prev
  });

  it('with no anchor (-1), falls back to scroll-based pickTurnIndex', () => {
    // Identical to calling pickTurnIndex directly — preserves first-press-from-scroll
    // and the mid-turn "prev snaps to current top" behavior.
    expect(pickTurnTarget(-1, TOPS, 0, 1, T)).toBe(pickTurnIndex(TOPS, 0, 1, T));
    expect(pickTurnTarget(-1, TOPS, 600, -1, T)).toBe(pickTurnIndex(TOPS, 600, -1, T));
  });
});
