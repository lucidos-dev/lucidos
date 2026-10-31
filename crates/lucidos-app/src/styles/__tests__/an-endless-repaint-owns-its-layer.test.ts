/**
 * An endless animation that repaints must sit on its own compositing layer.
 *
 * Only `transform` and `opacity` animate on the compositor. Any other property
 * (`background-position`, `background`, a colour) repaints every frame, and
 * without a layer of its own it re-rasterizes the whole layer it sits in. In a
 * transcript that is a viewport-sized layer. Under a streaming turn WebKit's GPU
 * process falls behind on those buffers and can grow without bound.
 *
 * `will-change: transform` gives the element a layer, so a frame repaints only
 * the element. It needs an atomic box: an inline span gets no layer from it.
 */
import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, relative, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import postcss, { type AtRule } from 'postcss';
import { cssRules, isReducedMotionRule, styleSheetPaths } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const srcRoot = resolve(here, '../..');
const sheets = styleSheetPaths(srcRoot).map((path: string) => ({
  path: relative(srcRoot, path) as string,
  css: readFileSync(path, 'utf8') as string,
}));

const COMPOSITED = new Set(['transform', 'translate', 'rotate', 'scale', 'opacity']);

/** Every property each `@keyframes` animates, across all its definitions. */
function keyframeProps(): Map<string, Set<string>> {
  const out = new Map<string, Set<string>>();
  for (const { css } of sheets) {
    postcss.parse(css).walkAtRules('keyframes', (at: AtRule) => {
      const props = out.get(at.params) ?? new Set<string>();
      at.walkDecls((d) => { props.add(d.prop); });
      out.set(at.params, props);
    });
  }
  return out;
}

interface EndlessRepaint { where: string; selector: string; keyframes: string; willChange: string | undefined }

function endlessRepaints(): EndlessRepaint[] {
  const frames = keyframeProps();
  const out: EndlessRepaint[] = [];
  for (const { path, css } of sheets) {
    for (const rule of cssRules(css)) {
      if (isReducedMotionRule(rule)) continue;
      const animation = rule.props.get('animation');
      if (!animation || !/\binfinite\b/.test(animation)) continue;
      const name = animation.split(/\s+/).find((token) => frames.has(token));
      if (!name) continue;
      const repaints = [...frames.get(name)!].some((prop) => !COMPOSITED.has(prop));
      if (repaints) {
        out.push({ where: path, selector: rule.selector, keyframes: name, willChange: rule.props.get('will-change') });
      }
    }
  }
  return out;
}

describe('an endless repaint owns its layer', () => {
  it('finds the running shimmer, so the scan is not vacuous', () => {
    expect(endlessRepaints().map((r) => r.keyframes)).toContain('running-shimmer');
  });

  it('declares will-change: transform beside every endless repainting animation', () => {
    const missing = endlessRepaints()
      .filter((r) => !/\btransform\b/.test(r.willChange ?? ''))
      .map((r) => `${r.where}: ${r.selector} (${r.keyframes})`);
    expect(missing, 'animate transform/opacity instead, or add will-change: transform on an atomic box').toEqual([]);
  });

  it('gives the running shimmer an atomic box, since it rides on inline spans', () => {
    const shimmer = sheets
      .flatMap(({ css }) => cssRules(css))
      .find((r) => r.selector === '.running-shimmer');
    expect(shimmer?.props.get('display')).toBe('inline-block');
  });
});
