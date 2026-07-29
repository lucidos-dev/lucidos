import { describe, it, expect } from 'vitest';
import { SwipeTouch } from './swipe';

describe('SwipeTouch', () => {
  it('starts dx at 0 when crossing the lock threshold (no dead-zone jump)', () => {
    // Without subtracting LOCK_THRESHOLD on lock, the first frame after the
    // 8px direction-lock jumps the image by 8px — visible as a "snap".
    const s = new SwipeTouch();
    s.start(0, 0);
    expect(s.move(7, 0)).toBeNull();          // below threshold, undecided
    expect(s.move(8, 0)).toBe(0);              // locks at threshold → start from 0
    expect(s.move(20, 0)).toBe(12);            // 20 - 8 = 12px of visible drag
  });

  it('returns null while moving vertically', () => {
    const s = new SwipeTouch();
    s.start(0, 0);
    expect(s.move(0, 20)).toBeNull();
    expect(s.move(5, 30)).toBeNull();
  });

  it('commits a fast horizontal swipe to the right as previous', () => {
    const s = new SwipeTouch();
    s.start(0, 0);
    s.move(50, 0);
    expect(s.end(800)).toBe(-1);               // finger right → previous
  });
});
