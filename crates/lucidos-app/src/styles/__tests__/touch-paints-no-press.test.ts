/**
 * A tap paints no background. Hover belongs to a pointer, and a finger gets no
 * fill at all. Three ways a phone used to paint one:
 *
 * - WebKit's own grey tap highlight, on any control that did not opt out.
 * - A `:hover` outside `@media (hover: hover)`. On touch it latches onto the
 *   last element tapped and stays until the next tap elsewhere.
 * - An `:active` fill, which flashes under the finger.
 *
 * Press feedback that is not a fill (a scale, a squeeze) is still allowed.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { relative, resolve } from 'node:path';
import {
  cssRules, ENGINE_SERVED_CSS, REPO_ROOT, selectorList, styleSheetPaths, type CssRule,
} from './css-rule-helpers';

/** Every stylesheet in the client, component-level sheets included, plus the
 *  ones the engine serves to app frames. A finger taps inside an app too. */
const paths: string[] = [...new Set([
  ...styleSheetPaths(resolve(REPO_ROOT, 'crates/lucidos-app/src')),
  ...ENGINE_SERVED_CSS.map((path) => resolve(REPO_ROOT, path)),
])];

const sheets: { file: string; rules: CssRule[] }[] = paths.map((path) => ({
  file: relative(REPO_ROOT, path),
  rules: cssRules(readFileSync(path, 'utf-8')),
}));

const all = sheets.flatMap(({ file, rules }) => rules.map((rule) => ({ file, rule })));
const where = ({ file, rule }: { file: string; rule: CssRule }) => `${file}: ${rule.selector}`;

/** A hover member that repeats a plain member of its list only pins
 *  specificity. It paints the same on touch, latched or not. */
function isSpecificityPin(member: string, members: string[]): boolean {
  return members.includes(member.replace(/:hover/g, ''));
}

/** The JS touch gate: `main.tsx` marks a touch device `is-touch`. A rule
 *  scoped under this never applies on touch, so its hover cannot latch. */
const TOUCH_CLASS_GATE = 'body:not(.is-touch)';

const PAINT_PROPS = ['background', 'background-color', 'background-image', 'box-shadow'];

describe('a tap paints no background', () => {
  it('turns the tap highlight off once, on the root', () => {
    const setters = all.filter(({ rule }) => rule.props.has('-webkit-tap-highlight-color'));
    expect(setters.map(where)).toEqual(['crates/lucidos-app/src/styles/global/base.css: html']);
    expect(setters[0].rule.props.get('-webkit-tap-highlight-color')).toBe('transparent');
  });

  it('keeps every hover on a pointer', () => {
    const ungated = all.filter(({ rule }) => {
      if (rule.atRules.includes('(hover: hover)')) return false;
      const members = selectorList(rule.selector);
      return members.some((m) =>
        m.includes(':hover') && !isSpecificityPin(m, members) && !m.startsWith(TOUCH_CLASS_GATE));
    });
    expect(ungated.map(where)).toEqual([]);
  });

  it('paints no fill on press', () => {
    const fills = all.filter(({ rule }) =>
      rule.selector.includes(':active') && PAINT_PROPS.some((p) => rule.props.has(p)));
    expect(fills.map(where)).toEqual([]);
  });
});
