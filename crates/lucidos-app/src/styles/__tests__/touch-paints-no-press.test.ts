import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { relative, resolve } from 'node:path';
import {
  cssRules, ENGINE_SERVED_CSS, REPO_ROOT, selectorList, styleSheetPaths, type CssRule,
} from './css-rule-helpers';

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

function isSpecificityPin(member: string, members: string[]): boolean {
  return members.includes(member.replace(/:hover/g, ''));
}

const TOUCH_CLASS_GATE = 'body:not(.is-touch)';

const PAINT_PROPS = ['background', 'background-color', 'background-image', 'box-shadow'];

describe('a tap paints no background', () => {
  it('turns the tap highlight off once, on each root', () => {
    const setters = all.filter(({ rule }) => rule.props.has('-webkit-tap-highlight-color'));
    expect(setters.map(where)).toEqual([
      'crates/lucidos-app/src/styles/global/base.css: html',
      'crates/lucidos-agents-api/src/api/sdk_iframe.css: html',
    ]);
    for (const { rule } of setters) {
      expect(rule.props.get('-webkit-tap-highlight-color')).toBe('transparent');
    }
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
