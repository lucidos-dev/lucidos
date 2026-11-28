import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { block, decl } from './css-rule-helpers';
import { REDUCED_MOTION_DURATION_SCALE } from '@lucidos/appearance';

const here = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(here, '../../../../..');

const baseCss = readFileSync(resolve(here, '../global/base.css'), 'utf8');
const iframeCss = readFileSync(
  resolve(REPO_ROOT, 'crates/lucidos-agents-api/src/api/sdk_iframe.css'), 'utf8',
);
const effectsSrc = readFileSync(resolve(here, '../../store/effects.ts'), 'utf8');

const DURATIONS: Record<string, string> = {
  '--duration-fast': '0.15s',
  '--duration-normal': '0.2s',
  '--duration-slow': '0.3s',
  '--duration-emphasis': '0.5s',
};

describe('animation-speed scale reaches every duration token', () => {
  const root = block(baseCss, ':root');

  it('defaults the scale to 1, so an untouched slider changes nothing', () => {
    expect(decl(root, '--duration-scale')).toBe('1');
  });

  for (const [token, oneX] of Object.entries(DURATIONS)) {
    it(`${token} is ${oneX} times the scale`, () => {
      expect(decl(root, token)).toBe(`calc(${oneX} * var(--duration-scale))`);
    });
  }

  it('has no duration token outside the scaled family', () => {
    const declared = [...root.matchAll(/(--duration-[\w-]+):/g)].map(m => m[1]);
    expect(new Set(declared)).toEqual(new Set(['--duration-scale', ...Object.keys(DURATIONS)]));
  });

  it('is published from the slider by store/effects.ts', () => {
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
    expect(decl(root, '--duration-scale')).toBe('1');
  });

  it('collapses the scale under reduced motion, to the value the host uses', () => {
    const reduce = block(iframeCss, ':root[data-motion="reduce"]');
    expect(decl(reduce, '--duration-scale')).toBe(String(REDUCED_MOTION_DURATION_SCALE));
  });
});
