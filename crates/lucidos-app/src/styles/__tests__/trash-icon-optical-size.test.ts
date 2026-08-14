/**
 * The trash's optical correction stays internally consistent, and the nominal
 * size it is applied to stays a per-context knob.
 *
 * `.icon-btn.header-icon .trash-icon` grows the glyph by the ratio of two
 * measured fills, because the trash paints far less of its viewBox than the
 * pencil it sits beside and a shared BOX is not a shared SIZE (the same problem
 * as `prompt-mark-optical-size.test.ts`, and the same correction). Growing it
 * also thickens the stroke, which is in user units, so the rule divides the
 * stroke by the same ratio to hold the weight.
 *
 * The rendered SIZE is asserted against a real browser by
 * `e2e/trigger-group-icon-optics.spec.ts`. The rendered STROKE cannot be:
 * no DOM box includes stroke (`getBoundingClientRect` on an SVG element is the
 * geometry box, `getBBox({stroke: true})` is ignored by both engines), and the
 * declaration reads back as an unresolved `calc()` on Chromium. So this pins it
 * structurally instead: both halves must be expressed in terms of the SAME two
 * measurements, which is what stops one being retuned without the other.
 *
 * It also pins the two things the CSS assumes about the artwork: that the glyph
 * still carries the class the rule selects, and that it is still drawn at the
 * stroke-width the rule divides.
 *
 * The second describe covers what the `--icon-glyph` indirection is FOR: the
 * header band and a list row take different nominals off one shared box, and
 * only the glyph moves, never the 2.25rem tap target the box rule exists to
 * guarantee. It also pins the split as the SINGLE source of both sizes, since a
 * caller that reintroduces a local nominal is back to two places to look.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { cssRules, rulesTargeting, selectorList } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(resolve(here, '../global/host-components.css'), 'utf8');
const chatCss = readFileSync(resolve(here, '../chat/input-messages.css'), 'utf8');
const baseCss = readFileSync(resolve(here, '../global/base.css'), 'utf8');
const groupCss = readFileSync(resolve(here, '../skills.css'), 'utf8');
const icons = readFileSync(resolve(here, '../../components/shared/icons.tsx'), 'utf8');
const chatExchange = readFileSync(resolve(here, '../../components/chat/ChatExchange.tsx'), 'utf8');
const groupHeader = readFileSync(resolve(here, '../../components/triggers/TriggerGroupHeader.tsx'), 'utf8');

/** The correction rule, found by a member of its selector list so grouping it
 *  with the row-icon twin cannot make this lookup silently miss. */
const rule = cssRules(css).find(r =>
  selectorList(r.selector).includes('.icon-btn.header-icon .trash-icon'),
);

describe('the trash is sized by its ink', () => {
  it('has a rule that names both measurements', () => {
    expect(rule, 'no .icon-btn.header-icon .trash-icon rule').toBeDefined();
    const pencil = Number(rule!.props.get('--pencil-ink'));
    const trash = Number(rule!.props.get('--trash-ink'));
    expect(pencil, '--pencil-ink is not a number').toBeGreaterThan(0);
    expect(trash, '--trash-ink is not a number').toBeGreaterThan(0);
    // Both are extents within a 24-unit viewBox, and the correction only makes
    // sense while the trash is the smaller of the two.
    expect(pencil).toBeLessThanOrEqual(24);
    expect(trash).toBeLessThan(pencil);
  });

  it('grows the box by the ratio and takes the same ratio out of the stroke', () => {
    // Order matters in the arithmetic, and it is inverted between the two: the
    // size multiplies by pencil/trash, the stroke by trash/pencil. Matching the
    // shape rather than evaluating it keeps this a test of the COUPLING, which
    // is the thing a retune can break.
    for (const prop of ['width', 'height']) {
      expect(rule!.props.get(prop), `${prop} is not derived from the two fills`)
        .toMatch(/calc\(var\(--icon-glyph\) \* var\(--pencil-ink\) \/ var\(--trash-ink\)\)/);
    }
    expect(rule!.props.get('stroke-width'), 'the stroke does not divide by the same ratio')
      .toMatch(/calc\(2 \* var\(--trash-ink\) \/ var\(--pencil-ink\)\)/);
  });

  it('is aimed at artwork that still carries the class and the stroke it assumes', () => {
    const trashIcon = /export function TrashIcon\(\)[\s\S]*?\n}/.exec(icons)?.[0] ?? '';
    expect(trashIcon, 'TrashIcon not found in icons.tsx').not.toBe('');
    expect(trashIcon, 'the glyph lost the class the rule selects').toContain('class="trash-icon"');
    // The `2` the stroke calc divides is this literal. Change one, change both.
    expect(trashIcon, 'the artwork is no longer drawn at stroke-width 2')
      .toContain('stroke-width="2"');
  });
});

/** The `--icon-size-*` scale, read from its own source rather than restated
 *  here, so "a smaller step" is measured against the real tokens. Rem only:
 *  the numbers are compared to each other, so a step that moved to another
 *  unit must drop out and fail the lookup loudly rather than be weighed
 *  against a rem as if it were one. */
const steps = new Map<string, number>();
for (const r of cssRules(baseCss)) {
  for (const [prop, value] of r.props) {
    if (prop.startsWith('--icon-size-') && /^[\d.]+rem$/.test(value)) {
      steps.set(prop, parseFloat(value));
    }
  }
}

/** The rule that owns the 2.25rem tap target the two bands share. */
const boxRule = cssRules(css).find(r => {
  const members = selectorList(r.selector);
  return members.includes('.icon-btn.header-icon') && members.includes('.icon-btn.row-icon');
});

/** Every rule that sets a nominal for one of the two boxes, per band. */
const nominals = (className: string) =>
  rulesTargeting(css, className).filter(r => r.props.has('--icon-glyph'));
const headerNominals = nominals('header-icon');
const rowNominals = nominals('row-icon');

/** The `--icon-size-*` step a nominal rule names, or undefined if it is not on
 *  the scale at all. */
const step = (value: string) => /^var\((--icon-size-[\w-]+)\)$/.exec(value)?.[1];

/** Every rule in a caller's own sheet aimed at one of the two row buttons. */
const callerRules = [
  ...rulesTargeting(chatCss, 'queued-message-remove'),
  ...rulesTargeting(groupCss, 'trigger-group-delete'),
  ...rulesTargeting(groupCss, 'trigger-group-rename'),
];

describe('the nominal glyph size is declared per band, on the shared tap target', () => {
  it('keeps the tap target on one rule and puts a nominal on each class it names', () => {
    expect(boxRule, 'no shared .icon-btn.header-icon, .icon-btn.row-icon rule').toBeDefined();
    expect(boxRule!.props.get('width'), 'the shared rule is not the 2.25rem tap target').toBe('2.25rem');
    expect(boxRule!.props.get('height')).toBe('2.25rem');
    // The two bands take different nominals, so one riding the shared rule
    // would be a default that silently reaches both again.
    expect(boxRule!.props.has('--icon-glyph'), 'the shared rule sets one nominal for both bands').toBe(false);

    for (const [className, found] of [['header-icon', headerNominals], ['row-icon', rowNominals]] as const) {
      // Exactly one: a second in this sheet would be a silent second default,
      // and whichever lost would be dead code.
      expect(found.length, `.${className} does not declare exactly one nominal`).toBe(1);
      // On a selector the shared box rule itself names, which is what keeps a
      // nominal from landing on anything that is not one of these tap targets.
      expect(selectorList(boxRule!.selector), `.${className}'s nominal is not on a shared tap target`)
        .toContain(found[0].selector);
      // And the nominal rule moves the glyph only. The box was grown
      // deliberately after the trash was reported unhittable on mobile.
      expect([...found[0].props.keys()], `"${found[0].selector}" sets more than the nominal`)
        .toEqual(['--icon-glyph']);
    }

    // The nominals are only worth anything while the glyph actually reads them.
    for (const sel of ['.icon-btn.header-icon svg', '.icon-btn.row-icon svg']) {
      const svg = cssRules(css).find(r => selectorList(r.selector).includes(sel));
      expect(svg, `no ${sel} rule`).toBeDefined();
      expect(svg!.props.get('width'), `${sel} is not sized from the nominal`).toBe('var(--icon-glyph)');
      expect(svg!.props.get('height')).toBe('var(--icon-glyph)');
    }
  });

  it('gives the header band the large step and puts a row one step below it', () => {
    const header = step(headerNominals[0].props.get('--icon-glyph')!);
    const row = step(rowNominals[0].props.get('--icon-glyph')!);
    expect(header, 'the header nominal is not an --icon-size-* step').toBeDefined();
    expect(row, 'the row nominal is not an --icon-size-* step').toBeDefined();
    expect(header, 'the header nominal moved off the large step').toBe('--icon-size-lg');
    expect(steps.get(header!), `${header} is not a rem step in base.css`).toBeGreaterThan(0);
    expect(steps.get(row!), `${row} is not a rem step in base.css`).toBeGreaterThan(0);
    // The whole point of the split: a row glyph reads against the header icons
    // a few rows up, and the pencil paints more of its box than they do, so at
    // one nominal it looked a fifth too big (e2e/trigger-group-icon-optics.spec.ts
    // measures the pair's real ink).
    expect(steps.get(row!), `${row} is not smaller than ${header}`).toBeLessThan(steps.get(header!)!);
  });

  it('is the single source: no caller retunes it in its own sheet', () => {
    // Both row callers want the same step, so neither carries a local override
    // that would have to out-rank the shared rule on specificity.
    for (const r of callerRules) {
      expect(r.props.has('--icon-glyph'), `"${r.selector}" picked up a local nominal`).toBe(false);
      for (const prop of ['width', 'height', 'min-width', 'min-height', 'padding']) {
        expect(r.props.has(prop), `"${r.selector}" sets ${prop} on the tap target`).toBe(false);
      }
      for (const prop of r.props.keys()) {
        expect(prop.startsWith('padding-'), `"${r.selector}" sets ${prop} on the tap target`).toBe(false);
      }
    }

    // A caller only gets the nominal while it still wears the class.
    expect(chatExchange, 'the queued button no longer carries the row-icon class')
      .toContain('class="icon-btn row-icon queued-message-remove"');
    for (const name of ['trigger-group-rename', 'trigger-group-delete']) {
      expect(groupHeader, `${name} no longer carries the row-icon class`)
        .toContain(`class="icon-btn row-icon ${name}"`);
    }
  });
});
