/**
 * The Animation speed slider's scale: the computed, the TS helper, and the two
 * things that would silently undo them.
 *
 * `durationScale` is the reciprocal of `speedMultiplier`, published to CSS as
 * `--duration-scale` (every `--duration-*` token folds it in) and read in TS by
 * `scaledDurationMs`. The second half is the one that rots: a timer that exists
 * to outlive a CSS transition has no compile-time link to the token it mirrors,
 * so scaling the CSS while leaving the timer fixed desyncs the pair at any
 * setting but 1x. The call-site scan at the bottom is that link.
 */
import { describe, it, expect, afterEach, beforeAll } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { animationSpeed, durationScale, scaledDurationMs } from '../store';

const here = dirname(fileURLToPath(import.meta.url));
const read = (rel: string) => readFileSync(resolve(here, rel), 'utf8');

/** Every `--duration-scale` write the publishing effect makes, newest last.
 *  The test-setup stub discards them, so record before importing effects. */
const published: string[] = [];

beforeAll(async () => {
  const style = document.documentElement.style;
  const original = style.setProperty.bind(style);
  style.setProperty = (prop: string, value: string) => {
    if (prop === '--duration-scale') published.push(value);
    original(prop, value);
  };
  await import('../effects');
});

afterEach(() => {
  animationSpeed.value = 0;
});

describe('durationScale', () => {
  it('is 1 at the slider centre', () => {
    // The untouched-slider case: nobody who never opens the setting may notice
    // that any of this exists.
    expect(durationScale.value).toBe(1);
  });

  it('is the reciprocal of the speed', () => {
    animationSpeed.value = 10; // 10x faster
    expect(durationScale.value).toBeCloseTo(0.1, 10);
    animationSpeed.value = -10; // 10x slower
    expect(durationScale.value).toBeCloseTo(10, 10);
  });

  it('is published to CSS on every change', () => {
    // Without this the CSS side is inert: the token default holds and every
    // transition runs at 1x whatever the slider says.
    animationSpeed.value = -10;
    expect(published[published.length - 1]).toBe('10');
    animationSpeed.value = 10;
    expect(published[published.length - 1]).toBe('0.1');
  });
});

describe('scaledDurationMs', () => {
  it('returns the base duration unchanged at 1x', () => {
    expect(scaledDurationMs(300)).toBe(300);
  });

  it('stretches and compresses with the slider', () => {
    animationSpeed.value = -10;
    expect(scaledDurationMs(300)).toBeCloseTo(3000, 6);
    animationSpeed.value = 10;
    expect(scaledDurationMs(300)).toBeCloseTo(30, 6);
  });
});

/** Every timer that exists to outlive a CSS transition, with the expression it
 *  must use. Each keeps its safety slack OUTSIDE the scaled call: slack is a
 *  fixed margin, not animation, so scaling it too would balloon to a second of
 *  dead time at 0.1x and shrink below usefulness at 10x. */
const MIRRORING_TIMERS: Array<{ file: string; expr: RegExp; what: string }> = [
  {
    file: '../../components/layout/splitHelpers.ts',
    expr: /scaledDurationMs\(PANE_TRANSITION_MS\) \+ 100/,
    what: 'holds `pane-animate` on for the length of the pane geometry transition',
  },
  {
    file: '../../components/drawer/ThreadDrawer.tsx',
    expr: /useLingeringFlag\(visible, scaledDurationMs\(PANE_TRANSITION_MS\) \+ 50\)/,
    what: "keeps the drawer's list mounted through its width collapse",
  },
  {
    file: '../../components/layout/ContentPane.tsx',
    expr: /scaledDurationMs\(NAV_COVER_ANIM_MS\) \+ NAV_COVER_SLACK_MS/,
    what: 'unmounts the navigation cover after its clear animation',
  },
  {
    file: '../../components/chat/ThreadView.tsx',
    expr: /scaledDurationMs\(SKELETON_FADE_OUT_MS\) \+ SKELETON_FADE_SLACK_MS/,
    what: 'keeps the thread skeleton overlay mounted through its fade',
  },
  {
    file: '../../components/apps/AppUiInline.tsx',
    expr: /scaledDurationMs\(COVER_FADE_MS\) \+ COVER_FADE_SLACK_MS/,
    what: "keeps the app frame's load cover mounted through its fade",
  },
];

describe('timers that mirror a CSS duration', () => {
  for (const { file, expr, what } of MIRRORING_TIMERS) {
    it(`scales: ${file.split('/').pop()} (${what})`, () => {
      expect(
        read(file),
        `This timer mirrors a --duration-* token, and that token is scaled by the `
        + 'Animation speed slider. An unscaled timer fires partway through the '
        + 'transition it is supposed to outlive: at 0.1x the drawer body blanks '
        + 'mid-slide and a maximizing pane snaps the rest of the way.',
      ).toMatch(expr);
    });
  }
});
