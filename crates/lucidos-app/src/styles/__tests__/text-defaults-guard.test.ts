import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules, selectorList } from './css-rule-helpers';
import { docFamilyText } from '../../test-doc-family';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesRoot: string = resolve(here, '..');
const repoRoot: string = resolve(here, '../../../../..');

const baseCss: string = readFileSync(resolve(stylesRoot, 'global/base.css'), 'utf-8');
const sharedCss: string = readFileSync(resolve(stylesRoot, 'global/shared-components.css'), 'utf-8');
const iframeCss: string = readFileSync(
  resolve(repoRoot, 'crates/lucidos-agents-api/src/api/sdk_iframe.css'),
  'utf-8'
);
const typeScaleWalk: string = readFileSync(
  resolve(repoRoot, 'crates/lucidos-app/e2e/typeScaleWalk.ts'),
  'utf-8'
);

function scaleSteps(css: string): Map<string, string> {
  const out = new Map<string, string>();
  for (const m of css.matchAll(/--font-size-([\w]+):\s*([^;]+);/g)) {
    if (!out.has(m[1])) out.set(m[1], m[2].trim());
  }
  return out;
}

function ruleForSelectors(css: string, wanted: string[]) {
  return cssRules(css).find(rule => {
    const list = selectorList(rule.selector);
    return list.length === wanted.length && wanted.every(w => list.includes(w));
  });
}

describe('the text defaults that make an omission harmless', () => {
  it('sets the body step on body, so unsized text is prose and not a heading', () => {
    const body = cssRules(baseCss).find(r => r.selector === 'body' && !r.atRules);
    expect(body, 'the body rule in base.css is gone').toBeTruthy();
    expect(body!.props.get('font-size')).toBe('var(--font-size-md)');
  });

  it('keeps the root on the ui-scale, which is what every rem step tracks', () => {
    const html = cssRules(baseCss).find(
      r => r.selector === 'html' && !r.atRules && r.props.has('font-size')
    );
    expect(html, 'the html font-size rule in base.css is gone').toBeTruthy();
    expect(html!.props.get('font-size')).toContain('--user-ui-scale');
  });

  it('hands form controls back the family and size the UA shorthand took', () => {
    const controls = ruleForSelectors(baseCss, ['input', 'textarea', 'select', 'button']);
    expect(
      controls,
      'base.css has no `input, textarea, select, button` rule; a control now renders in the UA font at a fixed size'
    ).toBeTruthy();
    expect(controls!.props.get('font-family')).toBe('inherit');
    expect(controls!.props.get('font-size')).toBe('inherit');
  });

  it('uses longhands, because the `font` shorthand resets weight and features', () => {
    const controls = ruleForSelectors(baseCss, ['input', 'textarea', 'select', 'button']);
    expect(controls!.props.has('font')).toBe(false);
  });

  it('leaves html out of the control rule, or the ui-scale would be overridden', () => {
    const controls = ruleForSelectors(baseCss, ['input', 'textarea', 'select', 'button']);
    expect(selectorList(controls!.selector)).not.toContain('html');
  });

  it('still names the family on controls in the app iframe, buttons included', () => {
    const button = cssRules(iframeCss).find(r => r.selector === 'button' && !r.atRules);
    expect(button, 'the iframe button rule is gone').toBeTruthy();
    expect(button!.props.get('font-family')).toBe('var(--font-ui)');

    const fields = cssRules(iframeCss).find(
      r => r.selector === 'input, textarea, select' && !r.atRules
    );
    expect(fields!.props.get('font-family')).toBe('var(--font-ui)');

    const body = cssRules(iframeCss).find(r => r.selector === 'body' && !r.atRules);
    expect(body!.props.get('font-size')).toBe('var(--font-size-sm)');
    expect(fields!.props.get('font-size')).toBe('var(--font-size-md)');
    expect(button!.props.get('font-size')).toBe('var(--font-size-md)');
    const rowInfo = cssRules(iframeCss).find(r => r.selector === '.list-row-info' && !r.atRules);
    expect(rowInfo?.props.get('font-size')).toBe('var(--font-size-md)');
  });

  it('names the mono face on rendered code, which inherits none from body', () => {
    const code = cssRules(sharedCss).filter(r => r.selector === '.markdown-content code');
    expect(code.length, 'the .markdown-content code rule is gone').toBeGreaterThan(0);
    expect(code.some(r => r.props.get('font-family') === 'var(--font-mono)')).toBe(true);
  });
});

describe('the type scale itself', () => {
  const steps = scaleSteps(baseCss);

  it('is the closed set of ten steps the rules and docs name', () => {
    expect([...steps.keys()]).toEqual([
      '3xs', '2xs', 'xs', 'sm', 'md', 'lg', 'xl', '2xl', '3xl', 'display',
    ]);
  });

  it('anchors 1rem at xl, a heading, which is why an omission reads as too big', () => {
    expect(steps.get('xl')).toBe('1rem');
    expect(parseFloat(steps.get('md')!)).toBeLessThan(1);
  });

  it('is mirrored value-for-value into the sheet served to app iframes', () => {
    expect(scaleSteps(iframeCss)).toEqual(steps);
  });

  it('is quoted verbatim by the two docs that hand it to artifact authors', () => {
    for (const doc of [
      'system-knowhow/best-practices.md',
      '.claude/skills/lucidos-cli/SKILL.md',
    ]) {
      const quoted = scaleSteps(docFamilyText(repoRoot, doc));
      expect(quoted, `${doc} no longer quotes the type scale`).toEqual(steps);
    }
  });

  it('hands artifact authors the chat prose step as their body size', () => {
    for (const doc of [
      'system-knowhow/best-practices.md',
      '.claude/skills/lucidos-cli/SKILL.md',
    ]) {
      const text = docFamilyText(repoRoot, doc);
      expect(text, `${doc} no longer pastes the chat prose step on body`)
        .toContain('body { font-size: var(--font-size-sm); line-height: 1.5; }');
    }
  });

  it('matches the multipliers the rendered guard asserts against', () => {
    const declared = [...steps.values()].map(v => parseFloat(v)).sort((a, b) => a - b);
    const m = typeScaleWalk.match(/SCALE_STEPS = \[([^\]]+)\]/);
    expect(m, 'SCALE_STEPS not found in e2e/typeScaleWalk.ts').toBeTruthy();
    const inSpec = m![1].split(',').map((s: string) => parseFloat(s.trim())).sort((a, b) => a - b);
    expect(inSpec).toEqual(declared);
  });
});
