/**
 * **The two transcript-wide turn controls are ON unless the reader turned them off.**
 *
 * A reader who has never touched either sees the full response and the step
 * log, rather than the latest answer over a hidden one. Both are localStorage
 * signals, so "the default" is what an ABSENT key reads as, which is the one
 * line each of these seeds is.
 *
 * What storage holds is the other half, and the non-obvious one. The old keys
 * were written on every load, whether or not anyone clicked, so every browser
 * that has ever opened the app already holds `false` under
 * `lucidos-steps-expanded` / `lucidos-details-expanded`. Those recorded the OLD
 * default and nothing else: an eager write cannot be told apart from a
 * deliberate one. Reading them here would pin every existing reader to the
 * default this replaces, and the flip would only ever be visible in a fresh
 * browser profile. So the seeds read new keys, the stale pair is dropped
 * (taking a genuine "I turned steps off" with it, once), and `persistTurnControl`
 * stores only the deviation so the trap cannot be re-set for the next reader.
 */
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
    // `persistTurnControl` writes `'false'` and nothing else, so any other
    // value is corrupted or foreign, and falling back to the default is the
    // direction that shows the reader more rather than less.
    for (const junk of ['', '0', 'off', 'FALSE', 'null']) {
      expect(seedTurnControl(junk), junk).toBe(true);
    }
  });

  it('seeds from keys the old default was never written under', () => {
    // See the header: the legacy pair holds a `false` in every existing
    // browser, put there by the persisting effect rather than by a reader.
    for (const key of [STEPS_EXPANDED_KEY, DETAILS_EXPANDED_KEY]) {
      expect(key).not.toBe('lucidos-steps-expanded');
      expect(key).not.toBe('lucidos-details-expanded');
    }
    expect(STEPS_EXPANDED_KEY).not.toBe(DETAILS_EXPANDED_KEY);
  });

  it('starts both controls on in a browser with nothing stored', () => {
    // The seeds ran at import, against the empty per-worker storage stub
    // (`src/test-setup.ts`), which is the fresh-reader case.
    expect(stepsExpanded.value).toBe(true);
    expect(detailsExpanded.value).toBe(true);
  });

  it('stores an off control and stores NOTHING for an on one', () => {
    // The write half is what keeps a stored value meaning "the reader turned
    // this off". Recording the default instead is what made the old keys
    // unreadable, and the effect behind this runs on load as well as on a
    // click, so an ON write would land in every browser that opened the app.
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
