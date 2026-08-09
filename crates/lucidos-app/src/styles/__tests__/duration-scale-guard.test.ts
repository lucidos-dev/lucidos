/**
 * Every transition duration in the app is scaled by the Animation speed slider.
 *
 * The slider (Settings > System > Debugging) used to reach only the handful of
 * JS-driven animations that read `speedMultiplier` directly: the thread-row FLIP
 * and the toast stack. Every CSS transition read a static `--duration-*` literal
 * and ran at 1x whatever the slider said, which is why opening the thread drawer
 * and maximizing a pane ignored it: both are pure CSS on `--duration-slow`.
 *
 * The fix is at the token, so no transition can opt out: each `--duration-*` is
 * its 1x literal times `var(--duration-scale)`, which store/effects.ts publishes
 * onto `:root` as the reciprocal of the speed. That form is what this guard
 * pins. A new duration token declared as a bare literal would silently be the
 * one animation in the app that ignores the setting, and nothing else in the
 * gate parses CSS (`tsc` skips it; `vite build` only fails on syntax).
 *
 * The mirror in the engine's `sdk_iframe.css` is checked here too, for the
 * opposite property: it must keep the same literals (the two files' "keep in
 * sync" comments finally have teeth) while pinning the scale at 1, because a
 * custom property does not cross a document boundary and an app iframe is a
 * different document.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { block, decl } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(here, '../../../../..');

const baseCss = readFileSync(resolve(here, '../global/base.css'), 'utf8');
const iframeCss = readFileSync(
  resolve(REPO_ROOT, 'crates/lucidos-engine/src/api/sdk_iframe.css'), 'utf8',
);
const effectsSrc = readFileSync(resolve(here, '../../store/effects.ts'), 'utf8');

/** The duration family, with the value each token must resolve to at 1x. The
 *  same table holds for both sheets: an app's chrome is meant to animate like
 *  the host's, at the host's default speed. */
const DURATIONS: Record<string, string> = {
  '--duration-fast': '0.15s',
  '--duration-normal': '0.2s',
  '--duration-slow': '0.3s',
  '--duration-emphasis': '0.5s',
};

describe('animation-speed scale reaches every duration token', () => {
  const root = block(baseCss, ':root');

  it('defaults the scale to 1, so an untouched slider changes nothing', () => {
    // Also the value for the frame before the publishing effect first runs, and
    // for any document that renders this sheet without the bundle.
    expect(decl(root, '--duration-scale')).toBe('1');
  });

  for (const [token, oneX] of Object.entries(DURATIONS)) {
    it(`${token} is ${oneX} times the scale`, () => {
      expect(decl(root, token)).toBe(`calc(${oneX} * var(--duration-scale))`);
    });
  }

  it('has no duration token outside the scaled family', () => {
    // A token added as a bare literal is the failure this whole guard exists
    // for: it would be the one thing in the app the slider cannot slow down.
    const declared = [...root.matchAll(/(--duration-[\w-]+):/g)].map(m => m[1]);
    expect(new Set(declared)).toEqual(new Set(['--duration-scale', ...Object.keys(DURATIONS)]));
  });

  it('is published from the slider by store/effects.ts', () => {
    // The CSS side is inert without this: `--duration-scale` stays at its
    // default and every transition runs at 1x forever.
    expect(effectsSrc).toMatch(
      /setProperty\('--duration-scale', String\(durationScale\.value\)\)/,
    );
  });
});

describe('the app-iframe mirror', () => {
  const root = block(iframeCss, ':root');

  it('keeps the same 1x literals as the host', () => {
    for (const [token, oneX] of Object.entries(DURATIONS)) {
      expect(decl(root, token)).toBe(`calc(${oneX} * var(--duration-scale))`);
    }
  });

  it('pins the scale at 1, because the host cannot reach into an iframe', () => {
    // The host publishes the scale as an inline style on its OWN :root, and a
    // custom property does not cross a document boundary. Pinned rather than
    // plumbed on purpose: the slider is a host debugging aid for inspecting
    // host transitions, not a preference apps are expected to honour.
    expect(decl(root, '--duration-scale')).toBe('1');
  });
});
