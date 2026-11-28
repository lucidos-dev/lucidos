import { describe, it, expect } from 'vitest';
import { anchorAfterBackfill } from '../ThreadView';
import type { Exchange } from '../../../store/thread-events';


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

  it('answers null when the capture named no anchor at all', () => {
    expect(anchorAfterBackfill([ex('old-1')], pend(null, null))).toBeNull();
  });
});
