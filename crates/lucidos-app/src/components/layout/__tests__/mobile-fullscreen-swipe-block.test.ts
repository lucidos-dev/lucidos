import { describe, it, expect } from 'vitest';
import { shouldStartPaneSwipe } from '../MobileSwipeContainer';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

// ─────────────────────────────────────────────────────────────────────────────
// A pseudo-fullscreen app owns the whole screen: NO Lucidos swipe, of either
// kind, may run while it is up.
//
// Two kinds had to be stopped, and only one of them is ours:
//
//   1. The host's three-pane swipe. Nothing visible moves (the overlay is
//      position:fixed over the viewport and the track's transform is pinned to
//      none), so a pane change while fullscreen is invisible state drift: the
//      user exits fullscreen onto a pane they never chose.
//
//   2. WebKit's native back/forward edge gesture in the standalone iOS PWA.
//      This is the one the user actually hit. The host suppresses it by calling
//      preventDefault on the touchstart (shouldSuppressEdgeNavigation), which
//      requires the touch to REACH the host, which requires an element above
//      the app iframe at the screen edge. The `.edge-swipe-zone` strips do that
//      job in the normal layout, but they live in `.mobile-swipe-pane` and the
//      fullscreen overlay (z-index: var(--z-tooltip), inset 0) covers them, so
//      an edge touch went straight into the iframe, the host saw nothing, and
//      WebKit claimed the gesture: a swipe from the left edge navigated the
//      joint session history and surfaced a snapshot of the app from before it
//      went fullscreen. The fix re-hangs the same strips inside the overlay.
//
// The frontend test environment is deliberately non-jsdom, so the wiring is
// pinned in source, the same approach as AppUiInline.test.ts.
// ─────────────────────────────────────────────────────────────────────────────

const here: string = dirname(fileURLToPath(import.meta.url));
const swipeSrc = readFileSync(resolve(here, '../MobileSwipeContainer.tsx'), 'utf-8');
const zonesSrc = readFileSync(resolve(here, '../EdgeSwipeZones.tsx'), 'utf-8');
const appUiSrc = readFileSync(resolve(here, '../../apps/AppUiInline.tsx'), 'utf-8');
const previewsCss = readFileSync(resolve(here, '../../../styles/panels/previews.css'), 'utf-8');

describe('shouldStartPaneSwipe', () => {
  const base = { textInputFocused: false, targetScrollable: false, appFullscreen: false };

  it('starts a pane swipe for an ordinary touch', () => {
    expect(shouldStartPaneSwipe(base)).toBe(true);
  });

  // The bug: a fullscreen app is the whole screen, so the pane behind it is not
  // something the user can be navigating.
  it('never starts a pane swipe while an app is pseudo-fullscreen', () => {
    expect(shouldStartPaneSwipe({ ...base, appFullscreen: true })).toBe(false);
  });

  it('never starts a pane swipe while a text input is focused', () => {
    expect(shouldStartPaneSwipe({ ...base, textInputFocused: true })).toBe(false);
  });

  it('never starts a pane swipe on a horizontally scrollable target', () => {
    expect(shouldStartPaneSwipe({ ...base, targetScrollable: true })).toBe(false);
  });
});

describe('pane swipe is off while pseudo-fullscreen', () => {
  it('feeds the live fullscreen signal into the start decision', () => {
    expect(swipeSrc).toMatch(/appFullscreen:\s*appPseudoFullscreen\.value/);
  });

  // Order matters: the edge preventDefault must still run when fullscreen, or
  // suppressing our own swipe would hand the gesture straight to WebKit. The
  // fullscreen bail therefore sits AFTER the shouldSuppressEdgeNavigation block.
  it('still suppresses native edge navigation before bailing out', () => {
    const start = swipeSrc.match(/const onTouchStart[\s\S]*?\n {2}\}, \[\]\);/)?.[0] ?? '';
    expect(start).not.toBe('');
    const suppressAt = start.indexOf('shouldSuppressEdgeNavigation');
    const bailAt = start.indexOf('shouldStartPaneSwipe');
    expect(suppressAt).toBeGreaterThan(-1);
    expect(bailAt).toBeGreaterThan(suppressAt);
  });
});

describe('edge guards inside the pseudo-fullscreen overlay', () => {
  // Without these the app iframe is the topmost thing at the screen edge, the
  // host never sees the touchstart, and WebKit's native back gesture wins.
  it('mounts the shared edge strips inside the fullscreen overlay', () => {
    const overlay = appUiSrc.match(/\{isPseudo && \([\s\S]*?\n {6}\)\}/)?.[0] ?? '';
    expect(overlay).toMatch(/<EdgeSwipeZones \/>/);
  });

  // `.edge-swipe-zone` is sized and positioned only under the mobile media
  // query, and the gesture it guards against only exists there. Rendering the
  // strips on desktop would drop two unstyled divs into the flex column.
  it('mounts them on the mobile layout only', () => {
    expect(appUiSrc).toMatch(/layout === 'mobile' && <EdgeSwipeZones \/>/);
  });

  // Both sides, or the forward gesture at the right edge stays live.
  it('is the single definition of both strips', () => {
    expect(zonesSrc).toMatch(/edge-swipe-zone edge-swipe-left/);
    expect(zonesSrc).toMatch(/edge-swipe-zone edge-swipe-right/);
    // One home for the markup: a second literal is a copy that can drift.
    for (const src of [swipeSrc, appUiSrc]) {
      expect(src).not.toMatch(/class="edge-swipe-zone/);
    }
  });

  // The exit button sits within the right strip's 1.25rem. Equal z-index would
  // leave the winner to DOM order, so the button states its own rank.
  it('keeps the exit button above the guards', () => {
    const exit = previewsCss.match(/\.pseudo-fullscreen-exit\s*\{[^}]*\}/)?.[0] ?? '';
    const zIndex = Number(exit.match(/z-index:\s*(\d+)/)?.[1] ?? NaN);
    expect(zIndex).toBeGreaterThan(1);
  });
});
