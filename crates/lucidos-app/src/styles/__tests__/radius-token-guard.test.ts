/**
 * Every corner in the shell follows the theme's three shape tokens, so a theme
 * can square the whole UI:
 *
 *   - `--radius-control` for buttons, inputs, chips and rows;
 *   - `--radius-surface` for cards, panels, the composer and dialogs;
 *   - `--radius-round` for circles and pills.
 *
 * A component that wants a bigger or smaller corner scales a token with
 * `calc()`. A literal radius is a corner no theme can reach. That is how the
 * composer box stayed round under a theme that set both tokens to 0.
 *
 * Two exemptions stay round in every theme, a square one included: spinners,
 * and the badges on icons. A square count reads as a key cap, not a mark.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, relative, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules, rulesTargeting, selectorList, styleSheetPaths } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const src: string = resolve(here, '../..');

/** Spinners keep a true circle: a rotating square reads as a glitch. */
const SPINNERS = new Set(['.mini-spinner']);

/** Every badge class. They take ONE pill corner, in badges.css, which no
 *  radius token reaches. */
const BADGES = [
  '.badge',
  '.drawer-badge',
  '.section-count-badge',
  '.brand-menu-ws-badge',
  '.ws-picker-badge',
  '.thread-status-question-badge',
  '.system-attention-badge',
  '.brand-menu-refresh-badge',
];
/** Modifiers a badge always wears beside a base class above. A corner set
 *  through one of them would beat the shared rule just the same. */
const BADGE_MODIFIERS = [
  '.brand-badge',
  '.brand-badge-dot',
  '.brand-unread-badge',
  '.drawer-view-count',
  '.system-attention-badge-corner',
  '.filter-badge',
];
const BADGE_SHEET: string = resolve(src, 'styles/badges.css');
const isBadgeCornerRule = (path: string, selector: string): boolean =>
  path === BADGE_SHEET && selectorList(selector).every(s => BADGES.includes(s));

/** The corners of a radius value, split on top-level spaces and the slash. */
function corners(value: string): string[] {
  const out: string[] = [];
  let depth = 0;
  let current = '';
  for (const ch of value) {
    if (ch === '(') depth++;
    if (ch === ')') depth--;
    if (depth === 0 && (ch === ' ' || ch === '/')) {
      if (current) out.push(current);
      current = '';
    } else {
      current += ch;
    }
  }
  if (current) out.push(current);
  return out;
}

/** A corner a theme reaches: zero, inherited, or built on a radius token. */
const followsTheTheme = (corner: string): boolean =>
  corner === '0' || corner === 'inherit' || /var\(--[a-z-]*radius/.test(corner);

describe('shell radii', () => {
  it('read a radius token at every corner', () => {
    const literal: string[] = [];
    for (const path of styleSheetPaths(src)) {
      for (const rule of cssRules(readFileSync(path, 'utf-8'))) {
        if (SPINNERS.has(rule.selector)) continue;
        if (isBadgeCornerRule(path, rule.selector)) continue;
        for (const [prop, value] of rule.props) {
          if (!/^border(-[a-z]+)*-radius$/.test(prop)) continue;
          if (corners(value).every(followsTheTheme)) continue;
          literal.push(`${relative(src, path)} ${rule.selector} { ${prop}: ${value} }`);
        }
      }
    }
    expect(literal, 'route these through --radius-control, --radius-surface or --radius-round').toEqual([]);
  });

  it('keep each spinner round, so the exemption still names a real rule', () => {
    const sheets = styleSheetPaths(src).map(path => readFileSync(path, 'utf-8')).join('\n');
    const rules = cssRules(sheets);
    for (const spinner of SPINNERS) {
      const rule = rules.find(r => r.selector === spinner && r.props.has('border-radius'));
      expect(rule?.props.get('border-radius'), spinner).toBe('50%');
    }
  });

  it('round every badge into a pill, in one rule a theme cannot reach', () => {
    const corner = cssRules(readFileSync(BADGE_SHEET, 'utf-8'))
      .filter(rule => rule.props.has('border-radius'));
    expect(corner.map(rule => selectorList(rule.selector))).toEqual([BADGES]);
    expect(corner[0].props.get('border-radius')).toBe('999px');
  });

  it('leave no badge a corner of its own, which would hand it back to the theme', () => {
    const own: string[] = [];
    for (const path of styleSheetPaths(src)) {
      const css = readFileSync(path, 'utf-8');
      for (const badge of [...BADGES, ...BADGE_MODIFIERS]) {
        for (const rule of rulesTargeting(css, badge.slice(1))) {
          if (isBadgeCornerRule(path, rule.selector)) continue;
          if (![...rule.props.keys()].some(prop => /^border(-[a-z]+)*-radius$/.test(prop))) continue;
          own.push(`${relative(src, path)} ${rule.selector}`);
        }
      }
    }
    expect(own, 'drop these corners: badges.css rounds every badge').toEqual([]);
  });
});
