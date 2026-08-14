/**
 * The two declarations that make a MISSING `font-size` harmless.
 *
 * This app's type scale is anchored below the platform's: `--font-size-xl` is
 * exactly `1rem` and is labelled "section headings", while body text is
 * `--font-size-md` at `0.8125rem`. So the root font-size, which is the user's
 * ui-scale, is a step and a half ABOVE body. Anything that resolves all the way
 * up to it renders as a heading, which is why every instance of this bug is
 * reported as "the font is too big" rather than as "unstyled".
 *
 * Two separate paths reach the root, and each needs its own default:
 *
 *  1. `body` carried no `font-size`, so ordinary text in a surface that styled
 *     everything except its size fell straight through. That shipped in
 *     Settings > System > What's New, where the release notes rendered larger
 *     than the version heading above them.
 *  2. A form control inherits NOTHING from `body`: the UA stylesheet applies the
 *     `font` shorthand to it. That is why the app grew roughly twenty
 *     hand-written `font: inherit` declarations, one per control somebody
 *     noticed. `.welcome-dismiss` was one of the ones nobody had noticed: it
 *     sized itself from a token and still painted in Arial.
 *
 * A source scan rather than a rendered test, for the same reason the sibling
 * guards are: the defect is a MISSING declaration and jsdom resolves no
 * cascade. `e2e/type-scale.spec.ts` is the rendered half, and asserts the same
 * two defaults reach real pixels.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules, selectorList } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesRoot: string = resolve(here, '..');
const repoRoot: string = resolve(here, '../../../../..');

const baseCss: string = readFileSync(resolve(stylesRoot, 'global/base.css'), 'utf-8');
const sharedCss: string = readFileSync(resolve(stylesRoot, 'global/shared-components.css'), 'utf-8');
const iframeCss: string = readFileSync(
  resolve(repoRoot, 'crates/lucidos-engine/src/api/sdk_iframe.css'),
  'utf-8'
);
const typeScaleWalk: string = readFileSync(
  resolve(repoRoot, 'crates/lucidos-app/e2e/typeScaleWalk.ts'),
  'utf-8'
);

/** The type scale as authored, newest values read from the file rather than
 *  restated here: a table copied into a test drifts exactly like a table copied
 *  into a rule does. */
function scaleSteps(css: string): Map<string, string> {
  const out = new Map<string, string>();
  for (const m of css.matchAll(/--font-size-([\w]+):\s*([^;]+);/g)) {
    // First declaration wins: a `@media` override further down is a different
    // question from "what is the scale", and none exists today.
    if (!out.has(m[1])) out.set(m[1], m[2].trim());
  }
  return out;
}

/** The top-level rule whose selector list is exactly these element names. */
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
    // The shorthand would undo the `font-feature-settings` rule this sits beside
    // (handing Fira Code's ligatures back to every control) and clobber
    // deliberate weights. Both traps are recorded at the sites that hit them:
    // steps.css and skills.css.
    const controls = ruleForSelectors(baseCss, ['input', 'textarea', 'select', 'button']);
    expect(controls!.props.has('font')).toBe(false);
  });

  it('leaves html out of the control rule, or the ui-scale would be overridden', () => {
    // `font-size: inherit` on the root would beat the ui-scale declaration at
    // the top of the file: both are element selectors, and this rule is later.
    const controls = ruleForSelectors(baseCss, ['input', 'textarea', 'select', 'button']);
    expect(selectorList(controls!.selector)).not.toContain('html');
  });

  it('still names the family on controls in the app iframe, buttons included', () => {
    // Apps never load base.css, so the engine's sheet carries its own copy of
    // this default. Its `input, textarea, select` rule always did; `button` did
    // not, so an app's buttons painted in the system face while its inputs
    // painted in --font-ui, inside the same app.
    const button = cssRules(iframeCss).find(r => r.selector === 'button' && !r.atRules);
    expect(button, 'the iframe button rule is gone').toBeTruthy();
    expect(button!.props.get('font-family')).toBe('var(--font-ui)');

    const fields = cssRules(iframeCss).find(
      r => r.selector === 'input, textarea, select' && !r.atRules
    );
    expect(fields!.props.get('font-family')).toBe('var(--font-ui)');

    const body = cssRules(iframeCss).find(r => r.selector === 'body' && !r.atRules);
    expect(body!.props.get('font-size')).toBe('var(--font-size-md)');
  });

  it('names the mono face on rendered code, which inherits none from body', () => {
    // A `<code>` carries a UA `font-family: monospace`, so it inherits nothing
    // and painted in the browser's generic mono face rather than the app's
    // stack. Same class of miss as the two defaults above. Lives in
    // shared-components.css, so app iframes get the fix too.
    // Matched by selector rather than through `rulesTargeting`, whose subject is
    // the element a rule STYLES: here that is the `code`, not the container.
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
    // The premise the defaults above exist for. If this ever stops being true
    // the comments in base.css, the rule, and the knowhow all need rewriting,
    // so it fails here rather than going quietly out of date.
    expect(steps.get('xl')).toBe('1rem');
    expect(parseFloat(steps.get('md')!)).toBeLessThan(1);
  });

  it('is mirrored value-for-value into the sheet served to app iframes', () => {
    // Apps in the wild key off these exact values, and a shared class like
    // .list-row renders in both documents.
    expect(scaleSteps(iframeCss)).toEqual(steps);
  });

  it('is quoted verbatim by the two docs that hand it to artifact authors', () => {
    // A standalone HTML artifact links no Lucidos stylesheet, so its author has
    // to paste the scale. Both copies exist because the two audiences are
    // reached differently: a chat thread loads the knowhow, a coding-agent
    // session loads the skill at the moment it writes to artifacts/. Neither
    // can read base.css from a workspace with no checkout, which is why they
    // carry values at all, and this is what stops those values going stale.
    // The measured drift they exist to prevent was one full step UP on every
    // rung, so an approximate match is not good enough.
    for (const doc of [
      'system-knowhow/best-practices.md',
      '.claude/skills/lucidos-cli/SKILL.md',
    ]) {
      const quoted = scaleSteps(readFileSync(resolve(repoRoot, doc), 'utf-8'));
      expect(quoted, `${doc} no longer quotes the type scale`).toEqual(steps);
    }
  });

  it('matches the multipliers the rendered guard asserts against', () => {
    // e2e/typeScaleWalk.ts cannot import from here (different runner), so it
    // restates the scale as multipliers. This is the join that stops the two
    // drifting: a retune of the scale must update the walk in the same change.
    const declared = [...steps.values()].map(v => parseFloat(v)).sort((a, b) => a - b);
    const m = typeScaleWalk.match(/SCALE_STEPS = \[([^\]]+)\]/);
    expect(m, 'SCALE_STEPS not found in e2e/typeScaleWalk.ts').toBeTruthy();
    const inSpec = m![1].split(',').map((s: string) => parseFloat(s.trim())).sort((a, b) => a - b);
    expect(inSpec).toEqual(declared);
  });
});
