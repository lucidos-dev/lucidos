import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';
import { computeAppHeight, heldBandPx, keyboardBandMemory, keyboardBandPx } from '../MobileSwipeContainer';


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
    expect(swipeSource).toMatch(/lastSetHeight\s*=\s*-1[\s\S]*?setHeight\(/);
  });

  it('wake handler goes through the keyboard-aware computation (not a raw vv.height write)', () => {
    const onWakeBody = swipeSource.match(/const onWake = \(\) => \{([\s\S]*?)\};/)?.[1];
    expect(onWakeBody, 'onWake handler not found in MobileSwipeContainer.tsx').toBeTruthy();
    expect(onWakeBody!).toMatch(/currentAppHeight\(\)/);
    expect(onWakeBody!).not.toMatch(/onResize\(\)/);
  });

  it('wake handler blurs an iOS-preserved focus on a software-keyboard input', () => {
    const onWakeBody = swipeSource.match(/const onWake = \(\) => \{([\s\S]*?)\};/)?.[1];
    expect(onWakeBody, 'onWake handler not found in MobileSwipeContainer.tsx').toBeTruthy();
    expect(onWakeBody!).toMatch(/\.blur\(\)/);
  });

  it('wake-time blur is scoped to vv.height appearing shrunk relative to the layout viewport', () => {
    const onWakeBody = swipeSource.match(/const onWake = \(\) => \{([\s\S]*?)\};/)?.[1];
    expect(onWakeBody, 'onWake handler not found in MobileSwipeContainer.tsx').toBeTruthy();
    expect(onWakeBody!).toMatch(/viewportIsKeyboardShrunk\(vv\.height,\s*window\.innerHeight\)/);
  });

  it('wake-time blur also runs on a viewport with no reading, which cannot rule out a pinned keyboard', () => {
    const onWakeBody = swipeSource.match(/const onWake = \(\) => \{([\s\S]*?)\};/)?.[1];
    expect(onWakeBody, 'onWake handler not found in MobileSwipeContainer.tsx').toBeTruthy();
    expect(onWakeBody!).toMatch(/!readableVisualViewport\(\)\s*\|\|\s*viewportIsKeyboardShrunk/);
  });

  it('onResize, onLayoutViewportChange, and the initial write all route through computeAppHeight', () => {
    const callCount = (swipeSource.match(/computeAppHeight\s*\(/g) ?? []).length;
    expect(callCount).toBeGreaterThanOrEqual(2);
    expect(swipeSource).toMatch(/const currentAppHeight = \(\) =>/);
  });
});

describe('computeAppHeight', () => {
  it('returns vv.height when keyboard is up (vv shrunk + text input focused)', () => {
    expect(computeAppHeight({
      vvHeight: 500,
      innerHeight: 844,
      activeElementOpensKeyboard: true,
    })).toBe(500);
  });

  it('returns innerHeight when keyboard is down (vv == innerHeight, no focus)', () => {
    expect(computeAppHeight({
      vvHeight: 844,
      innerHeight: 844,
      activeElementOpensKeyboard: false,
    })).toBe(844);
  });

  it('returns innerHeight when vv.height is stale and shrunk but no input focused (post-wake artifact)', () => {
    expect(computeAppHeight({
      vvHeight: 500,
      innerHeight: 844,
      activeElementOpensKeyboard: false,
    })).toBe(844);
  });

  it('returns innerHeight when vv.height equals innerHeight even with a focused input', () => {
    expect(computeAppHeight({
      vvHeight: 844,
      innerHeight: 844,
      activeElementOpensKeyboard: true,
    })).toBe(844);
  });

  it('uses the 100px threshold — a 50px vv shrink with focus is NOT treated as keyboard', () => {
    expect(computeAppHeight({
      vvHeight: 800,
      innerHeight: 844,
      activeElementOpensKeyboard: true,
    })).toBe(844);
  });

  it('a 100px exact shrink with focus is at the boundary — not treated as keyboard', () => {
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

  it('a zero-height visual viewport keeps the layout height, even with focus', () => {
    expect(computeAppHeight({
      vvHeight: 0,
      innerHeight: 852,
      activeElementOpensKeyboard: true,
    })).toBe(852);
  });
});

describe('keyboardBandPx', () => {
  it('is the room the keys take, while they are up', () => {
    expect(keyboardBandPx({
      vvHeight: 430,
      innerHeight: 844,
      activeElementOpensKeyboard: true,
    })).toBe(414);
  });

  it('is zero with nothing focused, so no view gains a dead scroll zone', () => {
    expect(keyboardBandPx({
      vvHeight: 430,
      innerHeight: 844,
      activeElementOpensKeyboard: false,
    })).toBe(0);
  });

  it('is zero when the viewports agree', () => {
    expect(keyboardBandPx({
      vvHeight: 844,
      innerHeight: 844,
      activeElementOpensKeyboard: true,
    })).toBe(0);
  });

  it('shares computeAppHeight threshold, so a small shrink buys no slack', () => {
    expect(keyboardBandPx({
      vvHeight: 800,
      innerHeight: 844,
      activeElementOpensKeyboard: true,
    })).toBe(0);
  });

  it('never answers negative when the visual viewport reads taller', () => {
    expect(keyboardBandPx({
      vvHeight: 900,
      innerHeight: 844,
      activeElementOpensKeyboard: true,
    })).toBe(0);
  });

  it('is zero when the visual viewport reports no height', () => {
    expect(keyboardBandPx({
      vvHeight: 0,
      innerHeight: 852,
      activeElementOpensKeyboard: true,
    })).toBe(0);
  });
});

describe('keyboardBandMemory', () => {
  const portrait = { width: 390, height: 844 };
  const landscape = { width: 844, height: 390 };

  it('reserves the widest band seen in this orientation', () => {
    const memory = keyboardBandMemory();
    memory.note(300, portrait);
    memory.note(336, portrait);
    memory.note(0, portrait);
    expect(memory.reserve(portrait)).toBe(336);
  });

  it('never reserves a portrait keyboard on a landscape screen', () => {
    const memory = keyboardBandMemory();
    memory.note(336, portrait);
    expect(memory.reserve(landscape)).toBe(Math.round(390 * 0.45));
    memory.note(200, landscape);
    expect(memory.reserve(landscape)).toBe(200);
    expect(memory.reserve(portrait)).toBe(336);
  });
});

describe('heldBandPx', () => {
  const intoTheBand = { paddingPx: 365, scrollTop: 513, scrollHeight: 1325, clientHeight: 812 };

  it('keeps the padding the scroll position still needs when the band drops', () => {
    expect(heldBandPx({ ...intoTheBand, bandPx: 0 })).toBe(365);
  });

  it('keeps only the part of the band the reader is scrolled into', () => {
    expect(heldBandPx({ ...intoTheBand, scrollTop: 300, bandPx: 0 })).toBe(152);
  });

  it('holds nothing when the scroll position is inside the new range', () => {
    expect(heldBandPx({ ...intoTheBand, scrollTop: 148, bandPx: 0 })).toBe(0);
  });

  it('holds nothing for a scroller the band does not pad', () => {
    expect(heldBandPx({ ...intoTheBand, paddingPx: 0, bandPx: 0 })).toBe(0);
  });

  it('holds nothing once the live band covers what is needed', () => {
    expect(heldBandPx({ ...intoTheBand, bandPx: 365, heldPx: 365 })).toBe(0);
  });

  it('drains as the reader scrolls back toward the content', () => {
    expect(heldBandPx({ ...intoTheBand, scrollTop: 400, bandPx: 0, heldPx: 365 })).toBe(252);
  });

  it('lets go at the top, where no padding is needed', () => {
    expect(heldBandPx({ paddingPx: 365, scrollTop: 0, scrollHeight: 812, clientHeight: 812, bandPx: 0, heldPx: 365 })).toBe(0);
  });

  it('lets go once the content it anchored has shrunk', () => {
    const shrunk = { paddingPx: 365, scrollTop: 300, scrollHeight: 1000, clientHeight: 700 };
    expect(heldBandPx({ ...shrunk, bandPx: 0, heldPx: 365, anchorContentPx: 960 })).toBe(0);
  });

  it('keeps holding while the content it anchored is intact', () => {
    expect(heldBandPx({ ...intoTheBand, bandPx: 0, heldPx: 365, anchorContentPx: 960 })).toBe(365);
  });

  it('never grows back when the reader scrolls down again', () => {
    expect(heldBandPx({ ...intoTheBand, bandPx: 0, heldPx: 200 })).toBe(200);
  });
});

describe('the band reaches the scroll container', () => {
  const mobileCss: string = readFileSync(resolve(here, '../../../styles/mobile.css'), 'utf-8');

  it('is published as --keyboard-band', () => {
    expect(swipeSource).toMatch(/setProperty\(\s*'--keyboard-band'/);
  });

  it('is reserved at focus time, before the keys have animated in', () => {
    expect(swipeSource).toMatch(/addEventListener\('focusin', armBand/);
  });

  it('survives a move between two fields', () => {
    expect(swipeSource).toMatch(
      /const onFocusOut = \(e: FocusEvent\) => \{\s*if \(opensSoftwareKeyboard\(e\.relatedTarget\)\) return;/,
    );
  });

  it('is spent as padding inside the pane scroller, a hold included', () => {
    expect(mobileCss).toMatch(
      /\.mobile-swipe-pane \.content-pane-body \{\s*padding-bottom: max\(var\(--keyboard-band, 0px\), var\(--keyboard-band-hold, 0px\)\);/,
    );
  });

  it('holds the scroll anchors before it writes a lower band', () => {
    expect(swipeSource).toMatch(/holdScrollAnchors\(px\);\s*lastSetBand = px;\s*document\.documentElement\.style\.setProperty\('--keyboard-band'/);
  });
});
