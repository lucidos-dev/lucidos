import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(resolve(here, '../ImagePopup.tsx'), 'utf-8');

// Architecture: render every image at a fixed left=i*100% slot, translate the
// strip by -index*W. After a swipe commit, the animation lands at the new
// index's slot and the signal index updates without changing the transform —
// no transform reset, no src swap, no two/three-frame race that could flash a
// stale image (which is what the prev/current/next architecture suffered).
describe('image popup swipe carousel (signal-driven transform)', () => {
  it('renders every image, not just prev/current/next', () => {
    expect(source).toMatch(/state\.images\.map\(/);
    expect(source, 'data-pos slot architecture is the source of the flash bug; must be removed')
      .not.toMatch(/data-pos=/);
  });

  it('strip transform is driven by state.index via a layout effect (single source of truth)', () => {
    expect(source, 'transform must be reasserted from state.index after each render')
      .toMatch(/useLayoutEffect/);
    // The layout effect computes -state.index * W and writes it to the strip.
    expect(source).toMatch(/-\s*state\.index\s*\*/);
  });

  it('commit handler does NOT reset the strip transform — the animation lands at the new index position', () => {
    // The race that caused the flash was: timer rAF resets transform AND
    // updates index in the same callback. With the new architecture, the
    // commit animation's end value already equals the new index's rest
    // position, so no reset is needed.
    expect(source, 'no commit-rAF that wipes transform — that was the race').not.toMatch(
      /requestAnimationFrame\([\s\S]*?style\.transform\s*=\s*['"]['"][\s\S]*?step\(/,
    );
    expect(source, 'no commit-rAF that wipes transform — that was the race').not.toMatch(
      /requestAnimationFrame\([\s\S]*?step\([\s\S]*?style\.transform\s*=\s*['"]['"]/,
    );
  });

  it('commit waits for the animation to finish before updating the signal (transitionend, not a hand-rolled timer)', () => {
    // setTimeout(..., SWIPE_COMMIT_MS) drifts: if the timer fires early the
    // animation is still mid-flight and the index update happens against a
    // half-translated strip. transitionend fires exactly when the browser
    // says the animation is done.
    expect(source).toMatch(/transitionend/);
  });

  it('does not call .decode() — neighbour <img>s are mounted from the start, no bitmap swap on commit', () => {
    expect(source).not.toMatch(/\.decode\(\)/);
  });
});
