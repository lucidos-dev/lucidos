/**
 * A skeleton bar must show on whatever surface it sits on.
 *
 * The base `.sk-bar` rule paints two translucent stops, so a bar composites
 * over any background, `--bg-tertiary` included. A second copy of the gradient
 * on one surface is how an opaque bar would come back.
 *
 * So only the base rule may paint a bar. A surface that needs other colours
 * (the picker's brand gradient) sets `--sk-base` / `--sk-shine` and nothing else.
 */
import { describe, it, expect } from 'vitest';
import postcss from 'postcss';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';

const STYLES = resolve(dirname(fileURLToPath(import.meta.url)), '..');

function cssFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e: { name: string; isDirectory(): boolean }) =>
    e.isDirectory() ? cssFiles(resolve(dir, e.name)) : e.name.endsWith('.css') ? [resolve(dir, e.name)] : []);
}

const barRules = cssFiles(STYLES).flatMap((file) => {
  const rules: { where: string; selector: string; props: string[]; base: string }[] = [];
  postcss.parse(readFileSync(file, 'utf8')).walkRules((rule) => {
    if (!/\.sk-bar\b/.test(rule.selector)) return;
    const props: string[] = [];
    let base = '';
    rule.walkDecls((d) => {
      props.push(d.prop);
      if (d.prop === '--sk-base') base = d.value;
    });
    rules.push({ where: relative(STYLES, file), selector: rule.selector, props, base });
  });
  return rules;
});

describe('skeleton bars show on any surface', () => {
  it('has one base rule, with a translucent body', () => {
    const base = barRules.filter((r) => r.selector === '.sk-bar' && r.props.includes('background'));
    expect(base.map((r) => r.where)).toEqual(['components.css']);
    expect(base[0]!.base).toMatch(/transparent/);
  });

  it('lets a scoped rule set only the two bar colours', () => {
    const offenders = barRules
      .filter((r) => r.selector !== '.sk-bar' && r.props.some((p) => p.startsWith('background')))
      .map((r) => `${r.where}: ${r.selector}`);
    expect(offenders, 'override --sk-base / --sk-shine instead of repainting the bar').toEqual([]);
  });
});
