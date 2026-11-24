import { describe, it, expect } from 'vitest';
import { shouldStartPaneSwipe } from '../MobileSwipeContainer';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';


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

describe('only a field the user types in holds off a pane swipe', () => {
  it('asks whether the focused element opens the keyboard', () => {
    expect(swipeSrc.match(/textInputFocused: opensSoftwareKeyboard\(document\.activeElement\)/g)?.length).toBe(1);
    expect(swipeSrc).toMatch(/const textInputFocused = opensSoftwareKeyboard\(document\.activeElement\)/);
  });

  it('never asks isTextInput, which counts a focused checkbox as typing', () => {
    expect(swipeSrc).not.toMatch(/isTextInput\(/);
  });
});

describe('pane swipe is off while pseudo-fullscreen', () => {
  it('feeds the live fullscreen signal into the start decision', () => {
    expect(swipeSrc).toMatch(/appFullscreen:\s*appPseudoFullscreen\.value/);
  });

  it('still suppresses native edge navigation before bailing out', () => {
    const start = swipeSrc.match(/function gateStripTouch[\s\S]*?\n\}\n/)?.[0] ?? '';
    expect(start).not.toBe('');
    const suppressAt = start.indexOf('shouldSuppressEdgeNavigation');
    const bailAt = start.indexOf('shouldStartPaneSwipe');
    expect(suppressAt).toBeGreaterThan(-1);
    expect(bailAt).toBeGreaterThan(suppressAt);
  });
});

describe('edge guards inside the pseudo-fullscreen overlay', () => {
  it('mounts the shared edge strips inside the fullscreen overlay', () => {
    const overlay = appUiSrc.match(/\{isPseudo && \([\s\S]*?\n {6}\)\}/)?.[0] ?? '';
    expect(overlay).toMatch(/<EdgeSwipeZones \/>/);
  });

  it('mounts them on the mobile layout only', () => {
    expect(appUiSrc).toMatch(/layout === 'mobile' && <EdgeSwipeZones \/>/);
  });

  it('is the single definition of both strips', () => {
    expect(zonesSrc).toMatch(/edge-swipe-zone edge-swipe-left/);
    expect(zonesSrc).toMatch(/edge-swipe-zone edge-swipe-right/);
    for (const src of [swipeSrc, appUiSrc]) {
      expect(src).not.toMatch(/class="edge-swipe-zone/);
    }
  });

  it('keeps the exit button above the guards', () => {
    const exit = previewsCss.match(/\.pseudo-fullscreen-exit\s*\{[^}]*\}/)?.[0] ?? '';
    const zIndex = Number(exit.match(/z-index:\s*(\d+)/)?.[1] ?? NaN);
    expect(zIndex).toBeGreaterThan(1);
  });
});
