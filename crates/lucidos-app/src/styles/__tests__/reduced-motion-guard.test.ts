/**
 * Reduced motion has ONE source of truth, and every animation answers to it.
 *
 * The client resolves the device's Motion setting and the OS switch into one
 * value (`utils/motion.ts`) and publishes it as `data-motion` on `<html>`. The
 * boot script does the same before first paint. So:
 *
 *   1. Nothing reads the media query itself. A stylesheet that does ignores a
 *      user who picked Reduce or Full in the app.
 *   2. Every animation that the duration scale cannot reach has a rule on
 *      `:root[data-motion="reduce"]` for its own selector. The scale collapses
 *      a scaled one-shot; an indefinite or literal-duration one needs a rule.
 *   3. The boot splash resolves before it paints, so it never moves for a
 *      device that asked for calm.
 */
import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, relative, resolve } from 'node:path';
import postcss from 'postcss';
import {
  REDUCED_MOTION_ROOT, clientSourcePaths, cssRules, isReducedMotionRule, selectorList,
  styleSheetPaths,
} from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const srcRoot = resolve(here, '../..');
const appRoot = resolve(srcRoot, '..');
const indexHtml = readFileSync(resolve(appRoot, 'index.html'), 'utf8');

/** The boot splash's inline stylesheet, which ships in index.html. */
function splashCss(): string {
  const start = indexHtml.indexOf('/* lucidos-boot-splash-css-start */');
  const end = indexHtml.indexOf('/* lucidos-boot-splash-css-end */');
  return indexHtml.slice(start, end);
}

/** Every stylesheet the client ships, as [label, css]. */
function clientStyleSheets(): Array<[string, string]> {
  const sheets: Array<[string, string]> = styleSheetPaths(srcRoot).map(
    (path: string) => [relative(srcRoot, path), readFileSync(path, 'utf8')] as [string, string],
  );
  sheets.push(['index.html (boot splash)', splashCss()]);
  return sheets;
}

/** One entry of an `animation` shorthand needs its own reduced-motion rule
 *  unless the duration scale reaches it. An indefinite one never scales, by
 *  convention (`.claude/rules/frontend-css.md`), and a literal one never can. */
function needsOwnRule(entry: string): boolean {
  return /\binfinite\b/.test(entry) || !/var\(--duration/.test(entry);
}

/** The animated selectors in `css` that no reduced-motion rule stops. */
function uncoveredAnimations(css: string): string[] {
  const rules = cssRules(css).filter((r) => !r.atRules.includes('@keyframes'));
  const stopped = new Set<string>();
  for (const rule of rules.filter(isReducedMotionRule)) {
    const stops = rule.props.get('animation') === 'none'
      || rule.props.get('animation-name') === 'none'
      || rule.props.has('animation-duration');
    if (!stops) continue;
    for (const member of selectorList(rule.selector)) stopped.add(member);
  }
  const missing: string[] = [];
  for (const rule of rules) {
    if (isReducedMotionRule(rule)) continue;
    const animation = rule.props.get('animation');
    if (!animation || animation === 'none') continue;
    if (!postcss.list.comma(animation).some(needsOwnRule)) continue;
    for (const member of selectorList(rule.selector)) {
      if (!stopped.has(`${REDUCED_MOTION_ROOT} ${member}`)) missing.push(member);
    }
  }
  return missing;
}

describe('one source of truth for reduced motion', () => {
  it('no stylesheet reads the media query itself', () => {
    const offenders = clientStyleSheets()
      .filter(([, css]) => css.includes('prefers-reduced-motion'))
      .map(([label]) => label);
    expect(
      offenders,
      'Key the rule on :root[data-motion="reduce"]. The media query ignores the in-app setting.',
    ).toEqual([]);
  });

  it('no script reads the media query itself', () => {
    const offenders = clientSourcePaths(srcRoot)
      .filter((path) => readFileSync(path, 'utf8').includes('prefers-reduced-motion'))
      .map((path) => relative(srcRoot, path));
    expect(
      offenders,
      'Read isReducedMotion() from utils/motion.ts. It folds in the in-app setting.',
    ).toEqual([]);
  });
});

describe('every animation has a reduced-motion path', () => {
  for (const [label, css] of clientStyleSheets()) {
    it(label, () => {
      expect(
        uncoveredAnimations(css),
        'Each needs a :root[data-motion="reduce"] rule for the same selector that '
        + 'sets `animation: none` (or a plain fade).',
      ).toEqual([]);
    });
  }

  it('flags an indefinite animation with no rule, so the check can fail', () => {
    const css = `.spinner { animation: spin 1s linear infinite; }
      .fade { animation: fade-in var(--duration-fast) ease; }`;
    expect(uncoveredAnimations(css)).toEqual(['.spinner']);
    const covered = `${css} ${REDUCED_MOTION_ROOT} .spinner { animation: none; }`;
    expect(uncoveredAnimations(covered)).toEqual([]);
  });
});

describe('the boot splash resolves motion before it paints', () => {
  it('runs the boot script, which sets data-motion, ahead of the splash markup', () => {
    const boot = indexHtml.indexOf('<!-- lucidos:appearance-boot -->');
    const splash = indexHtml.indexOf('<div class="boot-splash"');
    expect(boot).toBeGreaterThan(-1);
    expect(boot).toBeLessThan(splash);
    const bundle = readFileSync(
      resolve(appRoot, '../../packages/lucidos-sdk/src/generated/appearance-boot.host.js'),
      'utf8',
    );
    expect(bundle).toContain('data-motion');
  });
});
