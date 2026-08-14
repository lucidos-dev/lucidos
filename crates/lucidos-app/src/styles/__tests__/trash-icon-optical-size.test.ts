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
 * The second describe covers what the `--icon-glyph` indirection is FOR. Three
 * bands take three nominals off one shared box: a header band, a list row's
 * action cluster, and an icon inline in a run of text. Only the glyph moves,
 * never the 2.25rem tap target the box rule exists to guarantee. It also pins
 * the split as the SINGLE source of all three sizes: a caller that reintroduces
 * a local nominal is back to two places to look.
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

/** The three bands, widest nominal first. The order is asserted below. */
const BANDS = ['header-icon', 'row-icon', 'inline-icon'];

/** Parsed once and shared. Every lookup below reads this sheet, and each call
 *  into the helpers would otherwise re-parse the whole of it. */
const hostRules = cssRules(css);

/** The correction rule, found by a member of its selector list so grouping it
 *  with the other bands' twins cannot make this lookup silently miss. */
const rule = hostRules.find(r =>
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

  it('applies to the trash in every band', () => {
    // The correction is what makes one nominal mean one rendered height across
    // the set. A band left off this list ships an uncorrected trash, a fifth
    // short, which is the defect the rule was added for.
    for (const band of BANDS) {
      expect(selectorList(rule!.selector), `.${band}'s trash is not corrected`)
        .toContain(`.icon-btn.${band} .trash-icon`);
    }
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

/** The rule that owns the 2.25rem tap target the three bands share. */
const boxRule = hostRules.find(r => {
  const members = selectorList(r.selector);
  return BANDS.every(band => members.includes(`.icon-btn.${band}`));
});

/** Every rule that sets a nominal for one of the three boxes, keyed by band.
 *  Both tests below read this one map, so neither can derive a different
 *  answer from the other. */
const bandNominals = new Map(
  BANDS.map(band => [
    band,
    rulesTargeting(css, band).filter(r => r.props.has('--icon-glyph')),
  ]),
);

/** The `--icon-size-*` step a nominal rule names, or undefined if it is not on
 *  the scale at all. */
const step = (value: string) => /^var\((--icon-size-[\w-]+)\)$/.exec(value)?.[1];

/** Every rule in a caller's own sheet aimed at one of the three buttons. */
const callerRules = [
  ...rulesTargeting(chatCss, 'queued-message-remove'),
  ...rulesTargeting(groupCss, 'trigger-group-delete'),
  ...rulesTargeting(groupCss, 'trigger-group-rename'),
];

describe('the nominal glyph size is declared per band, on the shared tap target', () => {
  it('keeps the tap target on one rule and puts a nominal on each class it names', () => {
    expect(boxRule, 'no rule shares the tap target across all three bands').toBeDefined();
    expect(boxRule!.props.get('width'), 'the shared rule is not the 2.25rem tap target').toBe('2.25rem');
    expect(boxRule!.props.get('height')).toBe('2.25rem');
    // The bands take different nominals, so one riding the shared rule would be
    // a default that silently reaches all of them again.
    expect(boxRule!.props.has('--icon-glyph'), 'the shared rule sets one nominal for every band').toBe(false);

    for (const band of BANDS) {
      const found = bandNominals.get(band)!;
      // Exactly one: a second in this sheet would be a silent second default,
      // and whichever lost would be dead code.
      expect(found.length, `.${band} does not declare exactly one nominal`).toBe(1);
      // On a selector the shared box rule itself names, which is what keeps a
      // nominal from landing on anything that is not one of these tap targets.
      expect(selectorList(boxRule!.selector), `.${band}'s nominal is not on a shared tap target`)
        .toContain(found[0].selector);
      // And the nominal rule moves the glyph only. The box was grown
      // deliberately after the trash was reported unhittable on mobile.
      expect([...found[0].props.keys()], `"${found[0].selector}" sets more than the nominal`)
        .toEqual(['--icon-glyph']);
    }

    // The nominals are only worth anything while the glyph actually reads them.
    for (const band of BANDS) {
      const sel = `.icon-btn.${band} svg`;
      const svg = hostRules.find(r => selectorList(r.selector).includes(sel));
      expect(svg, `no ${sel} rule`).toBeDefined();
      expect(svg!.props.get('width'), `${sel} is not sized from the nominal`).toBe('var(--icon-glyph)');
      expect(svg!.props.get('height')).toBe('var(--icon-glyph)');
    }
  });

  it('starts the bands at the large step and takes one step down per band', () => {
    const named = BANDS.map(band => {
      const s = step(bandNominals.get(band)![0].props.get('--icon-glyph')!);
      expect(s, `the ${band} nominal is not an --icon-size-* step`).toBeDefined();
      expect(steps.get(s!), `${s} is not a rem step in base.css`).toBeGreaterThan(0);
      return s!;
    });
    expect(named[0], 'the header nominal moved off the large step').toBe('--icon-size-lg');
    // Strictly descending, which is the whole point of the split. A row glyph
    // reads against the header icons a few rows up. The pencil paints more of
    // its box than they do, so at the header nominal it looked a fifth too
    // big. An inline one reads against the type it interrupts, and steps down
    // again. The trash is wider than it is tall, which the height-matching
    // correction cannot see.
    for (let i = 1; i < named.length; i++) {
      expect(steps.get(named[i]), `${named[i]} is not smaller than ${named[i - 1]}`)
        .toBeLessThan(steps.get(named[i - 1])!);
    }
  });

  it('is the single source: no caller retunes it in its own sheet', () => {
    // A caller takes its band's step whole, so none carries a local override
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
    expect(chatExchange, 'the queued button no longer carries the inline-icon class')
      .toContain('class="icon-btn inline-icon queued-message-remove"');
    for (const name of ['trigger-group-rename', 'trigger-group-delete']) {
      expect(groupHeader, `${name} no longer carries the row-icon class`)
        .toContain(`class="icon-btn row-icon ${name}"`);
    }
  });
});
