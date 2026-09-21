import { describe, it, expect } from 'vitest';
import { anchorAfterBackfill } from '../ThreadView';
import type { Exchange } from '../../../store/thread-events';

/**
 * Where the window sits once a page of older history has folded in.
 *
 * The edge is an INDEX. A backfill grows the exchanges array at the front, so
 * the index has to be re-derived from something that survives the fold.
 *
 * The case that makes a single anchor insufficient: a page boundary rarely
 * lands on a turn start, so the oldest loaded turn is usually a fragment whose
 * opening message is still unfetched. When the page behind it arrives, the fold
 * merges the fragment into the real turn and the anchor's key stops existing.
 *
 * Plan: docs/plans/2026-09-20-a-long-thread-opens-without-its-whole-history.md
 */

/** An exchange as `exchangeKey` reads one: its boundary event's id. */
function ex(id: string): Exchange {
  return { userEvent: { _eventId: id }, userSeq: 1, steps: [] } as unknown as Exchange;
}

const pend = (anchorKey: string | null, nextKey: string | null, rowsHidden = 3) =>
  ({ anchorKey, nextKey, rowsHidden });

describe('anchorAfterBackfill', () => {
  it('re-points onto the anchor turn, keeping its hidden-row count', () => {
    const after = [ex('old-1'), ex('old-2'), ex('a'), ex('b')];
    expect(anchorAfterBackfill(after, pend('id:a', 'id:b'))).toEqual({
      exchange: 2,
      rowsHidden: 3,
    });
  });

  it('falls back to the turn BELOW when the boundary turn was absorbed', () => {
    // `a` was a fragment. Its opening message arrived with the page, so the
    // fold merged it into `old-2` and `id:a` no longer exists. The reader's
    // content is in `old-2`, the exchange just before `b`.
    const after = [ex('old-1'), ex('old-2'), ex('b')];
    expect(anchorAfterBackfill(after, pend('id:a', 'id:b'))).toEqual({
      exchange: 1,
      rowsHidden: 0,
    });
  });

  it('clamps the fallback at the first turn', () => {
    const after = [ex('b'), ex('c')];
    expect(anchorAfterBackfill(after, pend('id:a', 'id:b'))).toEqual({
      exchange: 0,
      rowsHidden: 0,
    });
  });

  it('answers null when the anchor is gone and there was no turn below it', () => {
    const after = [ex('old-1')];
    expect(anchorAfterBackfill(after, pend('id:a', null))).toBeNull();
  });

  it('answers null when neither turn survives', () => {
    const after = [ex('old-1'), ex('old-2')];
    expect(anchorAfterBackfill(after, pend('id:a', 'id:b'))).toBeNull();
  });

  /** A page that folded to nothing renderable leaves the window resting on no
   *  turn, so the capture names none. There is no reading position to hold, and
   *  the page behind it is pure gain. */
  it('answers null when the capture named no anchor at all', () => {
    expect(anchorAfterBackfill([ex('old-1')], pend(null, null))).toBeNull();
  });
});
