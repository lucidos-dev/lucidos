/**
 * An agent-built widget draws the widget shapes (`.ring`, `.bar-run`,
 * `.choice-row`) instead of a table. So they must follow the user's theme and
 * motion choice with nothing from the app. The file is served to every app
 * frame (`api/sdk.rs`) and documented in `system-knowhow/js-sdk.md`.
 */
import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { resolve } from 'node:path';
import {
  ENGINE_SERVED_CSS, REDUCED_MOTION_ROOT, REPO_ROOT, cssRules, isReducedMotionRule, selectorList,
} from './css-rule-helpers';

const SHAPES = 'crates/lucidos-engine/src/api/sdk_iframe_shapes.css';
const css: string = readFileSync(resolve(REPO_ROOT, SHAPES), 'utf8');
const styleRules = cssRules(css).filter((r) => !r.atRules.includes('@keyframes'));
const jsSdk: string = readFileSync(resolve(REPO_ROOT, 'system-knowhow/js-sdk.md'), 'utf8');

const THEME_RADIUS = /^var\(--radius-(round|surface|control)\)$/;
const LITERAL_COLOUR = /#[0-9a-f]{3,8}\b|\b(rgba?|hsla?|oklch|oklab|lab|lch|hwb)\(|\b(white|black)\b/i;
const LITERAL_DURATION = /(^|[\s,(])\d*\.?\d+m?s\b/;

describe('widget shapes', () => {
  it('ships to app frames', () => {
    expect(ENGINE_SERVED_CSS).toContain(SHAPES);
  });

  it('paints only theme colours', () => {
    const literal = styleRules.flatMap((r) => [...r.props]
      // A mask reads only alpha, so its opaque stop is no colour.
      .filter(([prop, value]) => !prop.startsWith('mask') && LITERAL_COLOUR.test(value))
      .map(([prop, value]) => `${r.selector} { ${prop}: ${value} }`));
    expect(literal).toEqual([]);
  });

  it('rounds every corner with a theme corner step', () => {
    const radii = styleRules.flatMap((r) => [...r.props]
      .filter(([prop]) => prop.includes('radius'))
      .map(([, value]) => value));
    expect(radii.length).toBeGreaterThan(0);
    expect(radii.filter((v) => !THEME_RADIUS.test(v))).toEqual([]);
  });

  it('times every transition and animation on a duration token', () => {
    const literal = styleRules.flatMap((r) => [...r.props]
      .filter(([prop, value]) => /^(transition|animation)/.test(prop) && LITERAL_DURATION.test(value))
      .map(([prop, value]) => `${r.selector} { ${prop}: ${value} }`));
    expect(literal).toEqual([]);
  });

  it('stops every animation under reduced motion', () => {
    const stopped = new Set(styleRules
      .filter((r) => isReducedMotionRule(r) && r.props.get('animation') === 'none')
      .flatMap((r) => selectorList(r.selector).map((s) => s.slice(REDUCED_MOTION_ROOT.length).trim())));
    const animated = styleRules
      .filter((r) => !isReducedMotionRule(r) && (r.props.get('animation') ?? 'none') !== 'none')
      .flatMap((r) => selectorList(r.selector));
    expect(animated.length).toBeGreaterThan(0);
    expect(animated.filter((s) => !stopped.has(s))).toEqual([]);
  });

  it('documents the defaults it declares', () => {
    const declared = (selector: string, prop: string): string | undefined =>
      styleRules.find((r) => r.selector === selector)?.props.get(prop);
    for (const [selector, prop] of [['.ring', '--ring-size'], ['.ring', '--ring-weight'], ['.bar-run', '--bar-run-height']]) {
      const value = declared(selector, prop);
      expect(value, `${selector} declares ${prop}`).toBeDefined();
      const documented = [`\`${prop}\` (default \`${value}\`)`, `\`${prop}\` (\`${value}\`)`];
      expect(documented.some((d) => jsSdk.includes(d)), `js-sdk.md gives ${prop} as ${value}`).toBe(true);
    }
    expect(jsSdk).toContain(`at least \`${declared('.choice', 'min-inline-size')}\` square`);
  });

  it('documents every class it defines in js-sdk.md', () => {
    const classes = new Set([...css.matchAll(/\.([a-z][a-z0-9-]*)/g)].map((m) => m[1]));
    for (const name of ['ring', 'bar-run', 'choice-row', 'choice']) expect(classes).toContain(name);
    const undocumented = [...classes].filter((name) => !jsSdk.includes(`.${name}`));
    expect(undocumented).toEqual([]);
  });
});
