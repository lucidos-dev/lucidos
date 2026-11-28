import { describe, expect, it } from 'vitest';
import {
  DETAILS_EXPANDED_KEY,
  STEPS_EXPANDED_KEY,
  detailsExpanded,
  persistTurnControl,
  seedTurnControl,
  stepsExpanded,
} from '../store';

describe('turn-control defaults', () => {
  it('shows everything to a reader who has never touched a control', () => {
    expect(seedTurnControl(null)).toBe(true);
  });

  it('keeps whichever state the reader last left', () => {
    expect(seedTurnControl('false')).toBe(false);
    expect(seedTurnControl('true')).toBe(true);
  });

  it('reads anything else as the default, never as off', () => {
    for (const junk of ['', '0', 'off', 'FALSE', 'null']) {
      expect(seedTurnControl(junk), junk).toBe(true);
    }
  });

  it('seeds from keys the old default was never written under', () => {
    for (const key of [STEPS_EXPANDED_KEY, DETAILS_EXPANDED_KEY]) {
      expect(key).not.toBe('lucidos-steps-expanded');
      expect(key).not.toBe('lucidos-details-expanded');
    }
    expect(STEPS_EXPANDED_KEY).not.toBe(DETAILS_EXPANDED_KEY);
  });

  it('starts both controls on in a browser with nothing stored', () => {
    expect(stepsExpanded.value).toBe(true);
    expect(detailsExpanded.value).toBe(true);
  });

  it('stores an off control and stores NOTHING for an on one', () => {
    try {
      persistTurnControl(STEPS_EXPANDED_KEY, false);
      expect(localStorage.getItem(STEPS_EXPANDED_KEY)).toBe('false');
      expect(seedTurnControl(localStorage.getItem(STEPS_EXPANDED_KEY))).toBe(false);

      persistTurnControl(STEPS_EXPANDED_KEY, true);
      expect(localStorage.getItem(STEPS_EXPANDED_KEY)).toBeNull();
      expect(seedTurnControl(localStorage.getItem(STEPS_EXPANDED_KEY))).toBe(true);
    } finally {
      localStorage.removeItem(STEPS_EXPANDED_KEY);
    }
  });
});
