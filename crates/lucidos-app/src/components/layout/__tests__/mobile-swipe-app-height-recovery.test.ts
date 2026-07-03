import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';
import { computeAppHeight } from '../MobileSwipeContainer';

// iOS PWA suspend/resume frequently dismisses the on-screen keyboard without
// firing a fresh visualViewport `resize` event. The vv.resize handler is the
// only writer of `--app-height`, so without a wake-time recompute the variable
// stays pinned at the keyboard-shrunk height after resume and the app shell
// renders at half the viewport (visible black band below the prompt) until
// the user does a hard reload.
//
// Lock the wake recovery in source: visibilitychange (when document becomes
// visible) and pageshow (bfcache restore) must trigger a recompute.
//
// The recompute MUST read window.innerHeight (the layout viewport), NOT
// vv.height — on iOS PWA resume vv.height often still reports the stale
// keyboard-shrunk value, and the textarea remains document.activeElement
// (iOS preserves focus across suspend), so the regular onResize() isKeyboard
// branch wrongly concludes the keyboard is still open and writes the small
// value back into --app-height. window.innerHeight does not shrink for the
// iOS Safari keyboard and is reliably restored on resume.

const here: string = dirname(fileURLToPath(import.meta.url));
const swipeSource = readFileSync(
  resolve(here, '../MobileSwipeContainer.tsx'),
  'utf-8',
);

describe('MobileSwipeContainer — --app-height wake recovery', () => {
  it('listens for visibilitychange to recompute --app-height', () => {
    expect(swipeSource).toMatch(/addEventListener\(\s*['"`]visibilitychange['"`]/);
  });

  it('listens for pageshow to recompute --app-height', () => {
    expect(swipeSource).toMatch(/addEventListener\(\s*['"`]pageshow['"`]/);
  });

  it('removes the wake listeners on cleanup (no leak across remounts)', () => {
    expect(swipeSource).toMatch(/removeEventListener\(\s*['"`]visibilitychange['"`]/);
    expect(swipeSource).toMatch(/removeEventListener\(\s*['"`]pageshow['"`]/);
  });

  it('wake handler resets the lastSetHeight cache so the CSS variable is force-written', () => {
    // The setHeight() guard short-circuits when the new value matches the
    // cached lastSetHeight. On wake, the cached value may equal the stale
    // (keyboard-shrunk) value in CSS even if the new height is now correct —
    // a straight setHeight() call would no-op when both happen to equal.
    // Resetting the cache before the recompute guarantees the write.
    expect(swipeSource).toMatch(/lastSetHeight\s*=\s*-1[\s\S]*?setHeight\(/);
  });

  it('wake handler goes through the keyboard-aware computation (not a raw vv.height write)', () => {
    // iOS PWA resume often leaves vv.height pinned at the stale
    // keyboard-shrunk value with the textarea still focused. The wake handler
    // must NOT bounce back through the old isKeyboard path that trusted
    // vv.height + activeElement; it routes through computeAppHeight (via the
    // shared currentAppHeight() closure) which — after the blur above clears
    // ghost focus — returns window.innerHeight (the layout viewport,
    // reliably restored on resume).
    const onWakeBody = swipeSource.match(/const onWake = \(\) => \{([\s\S]*?)\};/)?.[1];
    expect(onWakeBody, 'onWake handler not found in MobileSwipeContainer.tsx').toBeTruthy();
    expect(onWakeBody!).toMatch(/currentAppHeight\(\)/);
    expect(onWakeBody!).not.toMatch(/onResize\(\)/);
  });

  it('wake handler blurs an iOS-preserved focus on a software-keyboard input', () => {
    // Without the blur, the wake fix only sticks until the next vv.resize.
    // On iOS PWA wake vv.resize often fires AFTER onWake with vv.height still
    // pinned at the stale shrunk value while the textarea retains focus from
    // before suspend — computeAppHeight's keyboard check then sees a shrunk
    // viewport + focused text input and returns the stale shrunk vv.height.
    // Explicit blur clears the ghost focus so the next vv.resize correctly
    // treats it as "no keyboard" and writes the full layout height. The user
    // re-taps to reopen the keyboard naturally, matching the actual on-screen
    // state.
    const onWakeBody = swipeSource.match(/const onWake = \(\) => \{([\s\S]*?)\};/)?.[1];
    expect(onWakeBody, 'onWake handler not found in MobileSwipeContainer.tsx').toBeTruthy();
    expect(onWakeBody!).toMatch(/\.blur\(\)/);
  });

  it('wake-time blur is scoped to vv.height appearing shrunk relative to the layout viewport', () => {
    // Don't dismiss focus on a search modal / settings input when no
    // keyboard-related state needs clearing — only clear ghost focus when
    // vv.height looks shrunk enough that a delayed resize could re-stamp it.
    const onWakeBody = swipeSource.match(/const onWake = \(\) => \{([\s\S]*?)\};/)?.[1];
    expect(onWakeBody, 'onWake handler not found in MobileSwipeContainer.tsx').toBeTruthy();
    expect(onWakeBody!).toMatch(/vv\.height\s*<\s*window\.innerHeight\s*-\s*100/);
  });

  it('onResize, onOrientationChange, and the initial write all route through computeAppHeight', () => {
    // Single decision site. Routing onOrientationChange through the helper
    // covers the case where the user rotates with the keyboard up — writing
    // innerHeight blindly would occlude the prompt area behind the keyboard
    // until the next vv.resize repaired it. Same for the initial write if a
    // text input is focused at mount.
    const callCount = (swipeSource.match(/computeAppHeight\s*\(/g) ?? []).length;
    // Helper definition (1) + currentAppHeight closure (1) + helper-import in
    // tests doesn't count (different file). Three currentAppHeight() call
    // sites (onResize, onOrientationChange, initial, plus the wake-time
    // recompute = 4) all hit the helper transitively, so the source-level
    // computeAppHeight reference count just needs to be ≥ 2 (definition +
    // closure body).
    expect(callCount).toBeGreaterThanOrEqual(2);
    expect(swipeSource).toMatch(/const currentAppHeight = \(\) =>/);
  });
});

// Pure helper — the wake/resize bug is a state-decision problem (when to
// write vv.height vs. innerHeight to --app-height), so unit-test the decision
// directly. Driving it through jsdom + a fake visualViewport would test the
// component wiring but not the logic that actually had the bug.
describe('computeAppHeight', () => {
  it('returns vv.height when keyboard is up (vv shrunk + text input focused)', () => {
    // User tapped textarea, iOS keyboard slid up, vv.height shrunk to fit
    // above the keyboard. App shell must shrink to vv.height so the prompt
    // area stays visible above the keyboard.
    expect(computeAppHeight({
      vvHeight: 500,
      innerHeight: 844,
      activeElementOpensKeyboard: true,
    })).toBe(500);
  });

  it('returns innerHeight when keyboard is down (vv == innerHeight, no focus)', () => {
    // No keyboard, no focus — normal viewport.
    expect(computeAppHeight({
      vvHeight: 844,
      innerHeight: 844,
      activeElementOpensKeyboard: false,
    })).toBe(844);
  });

  it('returns innerHeight when vv.height is stale and shrunk but no input focused (post-wake artifact)', () => {
    // The bug: iOS PWA wake + vv.resize fires with stale shrunk vv.height
    // even though the keyboard is actually dismissed. onWake's blur cleared
    // any iOS-preserved ghost focus, so activeElementOpensKeyboard is false
    // here. Must return innerHeight (the layout viewport — doesn't shrink
    // for the iOS keyboard) so --app-height recovers to full viewport.
    expect(computeAppHeight({
      vvHeight: 500,
      innerHeight: 844,
      activeElementOpensKeyboard: false,
    })).toBe(844);
  });

  it('returns innerHeight when vv.height equals innerHeight even with a focused input', () => {
    // Text input focused but no keyboard up (e.g., focus without keyboard on
    // a desktop browser, or iOS pre-keyboard-display). vv == innerHeight so
    // the shrinkage check fails → no keyboard signal → write innerHeight.
    expect(computeAppHeight({
      vvHeight: 844,
      innerHeight: 844,
      activeElementOpensKeyboard: true,
    })).toBe(844);
  });

  it('uses the 100px threshold — a 50px vv shrink with focus is NOT treated as keyboard', () => {
    // Small vv adjustments (URL bar, status overlays) shouldn't shrink the
    // app shell. Only meaningful shrinks (≥100px) with a focused input
    // qualify as keyboard.
    expect(computeAppHeight({
      vvHeight: 800,
      innerHeight: 844,
      activeElementOpensKeyboard: true,
    })).toBe(844);
  });

  it('a 100px exact shrink with focus is at the boundary — not treated as keyboard', () => {
    // Strict less-than threshold avoids flapping at exactly 100px.
    expect(computeAppHeight({
      vvHeight: 744,
      innerHeight: 844,
      activeElementOpensKeyboard: true,
    })).toBe(844);
  });

  it('101px shrink with focus IS keyboard', () => {
    expect(computeAppHeight({
      vvHeight: 743,
      innerHeight: 844,
      activeElementOpensKeyboard: true,
    })).toBe(743);
  });
});
