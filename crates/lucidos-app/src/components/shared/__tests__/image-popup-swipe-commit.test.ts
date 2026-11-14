import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(resolve(here, '../ImagePopup.tsx'), 'utf-8');

// Architecture: each slide is positioned by its SHORTEST SIGNED DELTA from the
// current index (so wrap-around neighbours sit adjacent on screen). The strip
// rests at translateX(0); a swipe commit animates it by ±W in the swipe
// direction, then snaps back to 0 as the signal updates and slides reposition.
//
// The previous "left = i*100%, strip = -index*W" model could not wrap: a
// last→first commit set targetPx = 0 and dragged the strip backward through
// every intermediate image. shortestDelta puts slide 0 at +100% when c=3, so
// the forward commit slides it in from the right, as the user expects.
describe('image popup swipe carousel (wrap-aware)', () => {
  it('renders every image, not just prev/current/next', () => {
    expect(source).toMatch(/state\.images\.map\(/);
    expect(source, 'data-pos slot architecture is the source of the flash bug; must be removed')
      .not.toMatch(/data-pos=/);
  });

  it('slide positions are wrap-aware (shortest signed delta from current index)', () => {
    expect(source).toMatch(/shortestDelta/);
  });

  it('strip transform is 0 at rest — wrap is impossible if it depends on -index*W', () => {
    expect(source, 'no -state.index * transform — wrap requires position-based slides')
      .not.toMatch(/-\s*state\.index\s*\*/);
  });

  it('commit moves strip by exactly ±W in the swipe direction (no wrap detour)', () => {
    expect(source, 'commit target must be a one-slot translation (-result*W), not -newIndex*W')
      .toMatch(/-\s*result\s*\*\s*drag\.w/);
  });

  it('commit waits for the animation to finish before updating the signal (transitionend, not a hand-rolled timer)', () => {
    expect(source).toMatch(/transitionend/);
  });

  it('does not call .decode() — neighbour <img>s are mounted from the start, no bitmap swap on commit', () => {
    expect(source).not.toMatch(/\.decode\(\)/);
  });
});
