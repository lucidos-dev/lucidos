import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { relative, resolve } from 'node:path';
import {
  ENGINE_SERVED_CSS, REPO_ROOT, clientSourcePaths, cssRules, isReducedMotionRule, rulesTargeting,
  selectorList, styleSheetPaths, type CssRule,
} from './css-rule-helpers';

const SRC: string = resolve(REPO_ROOT, 'crates/lucidos-app/src');
const SHARED: string = resolve(SRC, 'styles/global/shared-components.css');
const SPIN = 'spin 1s linear infinite';
const SNAP_GATE = '@supports (width: round(1px, 1px))';
const BOX_SIDES = ['width', 'height'];
const BOX_OWNING = [...BOX_SIDES, 'position', 'display', 'overflow'];

const SPIN_USERS = [
  '.mini-spinner::after',
  '.app-header .content-refresh-btn:not([data-state="idle"]) svg:not(.refresh-check)',
  '.mobile-refresh-indicator:not([data-state="idle"]) svg:not(.refresh-check)',
  '.ws-picker-reload.is-reloading svg',
];

interface Located { path: string; rule: CssRule }

const sheets: [string, string][] = [
  ...styleSheetPaths(SRC),
  ...ENGINE_SERVED_CSS.map((path) => resolve(REPO_ROOT, path)),
].map((path: string) => [path, readFileSync(path, 'utf8')]);

const allRules: Located[] = sheets.flatMap(([path, css]) =>
  cssRules(css)
    .filter((rule) => !rule.atRules.includes('@keyframes'))
    .map((rule) => ({ path, rule })));

const spinnerRules = allRules.filter(({ rule }) => /\.mini-spinner(?![\w-])/.test(rule.selector));
const sharedRules = spinnerRules.filter(({ path }) => path === SHARED).map(({ rule }) => rule);
const where = ({ path, rule }: Located) => `${relative(SRC, path)}: ${rule.selector}`;
const only = (selector: string, atRules = ''): CssRule | undefined =>
  sharedRules.find((rule) => rule.selector === selector && rule.atRules === atRules);

describe('the shared spinner is a still ring with a solid arc turning on it', () => {
  it('draws both layers in one flat colour on the pseudo-elements of one empty element', () => {
    const layers = only('.mini-spinner::before, .mini-spinner::after');
    expect(layers?.props.get('content')).toBe("''");
    expect(layers?.props.get('background')).toBe('var(--spinner-color)');
    expect(only('.mini-spinner::before')?.props.get('mask')).toBe('var(--spinner-ring)');
    expect(only('.mini-spinner::after')?.props.has('background')).toBe(false);
  });

  it('cuts the arc out of the ring with a mask, so a translucent colour stays even', () => {
    const arc = only('.mini-spinner::after');
    expect(arc?.props.get('mask')).toBe('var(--spinner-ring), var(--spinner-arc-shape)');
    expect(arc?.props.get('-webkit-mask')).toBe(arc?.props.get('mask'));
    expect(arc?.props.get('mask-composite')).toBe('intersect, add, add, add');
    expect(arc?.props.get('-webkit-mask-composite')).toBe('source-in, source-over, source-over, source-over');
  });

  it('clips the turning layer to its circle, so WebKit paints no mask ghost outside the ring', () => {
    const clip = only('.mini-spinner::after')?.props.get('clip-path') ?? '';
    expect(clip).toBe('circle(calc(50% + var(--spinner-edge) / 2))');
    expect(only('.mini-spinner::before')?.props.has('clip-path')).toBe(false);
  });

  it('fades the ring with opacity, so it needs no color-mix() support', () => {
    const ring = only('.mini-spinner::before');
    const opacity = Number(ring?.props.get('opacity'));
    expect(opacity).toBeGreaterThan(0);
    expect(opacity).toBeLessThan(0.5);
    const mixed = sharedRules.filter((rule) => [...rule.props.values()].some((value) => value.includes('color-mix(')));
    expect(mixed.map((rule) => rule.selector)).toEqual([]);
  });

  it('draws both layers in the system text colour under forced colors', () => {
    const forced = only('.mini-spinner::before, .mini-spinner::after', '@media (forced-colors: active)');
    expect(forced?.props.get('forced-color-adjust')).toBe('none');
    expect(forced?.props.get('--spinner-color')).toBe('CanvasText');
  });

  it('keeps the ring still and turns only the arc', () => {
    const animated = sharedRules
      .filter((rule) => !isReducedMotionRule(rule))
      .filter((rule) => [...rule.props.keys()].some((prop) => prop.startsWith('animation')));
    expect(animated.map((rule) => rule.selector)).toEqual(['.mini-spinner::after']);
    expect(animated[0].props.get('animation')).toBe(SPIN);
  });

  it('turns a solid arc with a round cap at each end', () => {
    const shape = only('.mini-spinner')?.props.get('--spinner-arc-shape') ?? '';
    expect(shape).toContain('conic-gradient(#000 var(--spinner-arc), transparent 0)');
    expect(shape.match(/var\(--spinner-cap-dot\)/g)).toHaveLength(2);
  });

  it('puts the far cap where the arc ends', () => {
    const base = only('.mini-spinner');
    const arc = (parseFloat(base?.props.get('--spinner-arc') ?? '') * Math.PI) / 180;
    expect(Number(base?.props.get('--spinner-arc-sin'))).toBeCloseTo(Math.sin(arc), 3);
    expect(Number(base?.props.get('--spinner-arc-cos'))).toBeCloseTo(Math.cos(arc), 3);
  });

  it('sets no transform on a layer, which the keyframes would start from', () => {
    const transformed = spinnerRules.filter(({ rule }) => rule.props.has('transform'));
    expect(transformed.map(where)).toEqual([]);
  });

  it('derives the stroke from the size, never from an absolute weight alone', () => {
    const base = only('.mini-spinner');
    expect(Number(base?.props.get('--spinner-stroke-share'))).toBeGreaterThan(0);
    expect(base?.props.get('--spinner-stroke')).toContain('var(--spinner-stroke-share)');
    expect(base?.props.get('--spinner-ring')).toContain('var(--spinner-stroke)');
  });

  it('draws no border anywhere, so no side snaps to the pixel grid on its own', () => {
    const bordered = spinnerRules.filter(({ rule }) =>
      [...rule.props.keys()].some((prop) => prop.startsWith('border') && !prop.endsWith('radius')));
    expect(bordered.map(where)).toEqual([]);
  });

  it('snaps the box to whole pixels where round() exists, and sizes it plainly elsewhere', () => {
    for (const side of BOX_SIDES) {
      expect(only('.mini-spinner')?.props.get(side)).toBe('var(--spinner-size)');
      expect(only('.mini-spinner', SNAP_GATE)?.props.get(side)).toBe('round(var(--spinner-size), 1px)');
    }
  });

  it('is sized through --spinner-size at every site, never through width or height', () => {
    const sized = spinnerRules
      .filter(({ path, rule }) => !(path === SHARED && rule.selector === '.mini-spinner'))
      .filter(({ rule }) => BOX_SIDES.some((side) => rule.props.has(side)));
    expect(sized.map(where)).toEqual([]);
  });

  it('shares its element only with a class that leaves the box alone', () => {
    const companions = new Set<string>();
    for (const path of clientSourcePaths(SRC)) {
      const source: string = readFileSync(path, 'utf8');
      const lists = [...source.matchAll(/class="([^"]*)"/g)]
        .map((m) => m[1].split(/\s+/).filter(Boolean))
        .filter((names) => names.includes('mini-spinner'));
      const mentions = source.match(/mini-spinner(?![\w-])/g) ?? [];
      expect(lists.length, `${relative(SRC, path)}: name the spinner only in a static class="…"`)
        .toBe(mentions.length);
      for (const name of lists.flat()) {
        if (name !== 'mini-spinner') companions.add(name);
      }
    }
    expect(companions.size).toBeGreaterThan(0);
    const owning: string[] = [];
    for (const name of companions) {
      for (const [path, css] of sheets) {
        for (const rule of rulesTargeting(css, name)) {
          if (BOX_OWNING.some((prop) => rule.props.has(prop))) owning.push(where({ path, rule }));
        }
      }
    }
    expect(owning, 'draw the spinner as a child of that element instead').toEqual([]);
  });
});

describe('everything that turns shares one keyframes block and one speed', () => {
  const users = allRules
    .filter(({ rule }) => !isReducedMotionRule(rule))
    .filter(({ rule }) => /\bspin\b/.test(rule.props.get('animation') ?? rule.props.get('animation-name') ?? ''));

  it('lists every user of spin, so a new spinner cannot appear beside the shared one', () => {
    expect(users.flatMap(({ rule }) => selectorList(rule.selector)).sort()).toEqual([...SPIN_USERS].sort());
  });

  it('turns each of them once a second', () => {
    for (const user of users) {
      expect(user.rule.props.get('animation'), where(user)).toBe(SPIN);
    }
  });
});
