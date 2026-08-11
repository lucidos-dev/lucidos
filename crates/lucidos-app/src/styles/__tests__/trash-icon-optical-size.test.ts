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
 * The second describe covers what the `--icon-glyph` indirection is FOR: one
 * context (the queued message's trash) picks a smaller nominal while every
 * other caller keeps the default, and only the glyph moves, never the 2.25rem
 * tap target the box rule exists to guarantee.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { cssRules, rulesTargeting } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(resolve(here, '../global/host-components.css'), 'utf8');
const chatCss = readFileSync(resolve(here, '../chat/input-messages.css'), 'utf8');
const baseCss = readFileSync(resolve(here, '../global/base.css'), 'utf8');
const groupCss = readFileSync(resolve(here, '../skills.css'), 'utf8');
const icons = readFileSync(resolve(here, '../../components/shared/icons.tsx'), 'utf8');
const chatExchange = readFileSync(resolve(here, '../../components/chat/ChatExchange.tsx'), 'utf8');

/** The correction rule, found by a member of its selector list so grouping it
 *  with the row-icon twin cannot make this lookup silently miss. */
const rule = cssRules(css).find(r =>
  r.selector.split(',').some(s => s.trim() === '.icon-btn.header-icon .trash-icon'),
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

/** Classes in the SUBJECT of the selector-list member aiming at `className`,
 *  which is what the cascade weighs. Counted rather than assumed, so the
 *  comparison below is between two real specificities. */
function subjectClasses(selector: string, className: string): string[] {
  // Token-boundary, so `.row-icon` is not answered by a `.row-icon-something`
  // that happens to share its prefix.
  const token = new RegExp(`\\.${className}(?![\\w-])`);
  const one = selector.split(',').find(s => token.test(s)) ?? '';
  const subject = one.trim().split(/\s+/).pop() ?? '';
  return subject.match(/\.[\w-]+/g) ?? [];
}

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

/** Every rule that sets a nominal for a `.row-icon` box, in either sheet. */
const sharedNominals = rulesTargeting(css, 'row-icon').filter(r => r.props.has('--icon-glyph'));
const queuedRules = rulesTargeting(chatCss, 'queued-message-remove');
const queuedNominals = queuedRules.filter(r => r.props.has('--icon-glyph'));

describe('the nominal glyph size is a per-context knob', () => {
  it('defaults to the large step on the shared box, so the trigger group heading is unchanged', () => {
    // One declaration, on the box rule itself: a second one in this sheet would
    // be a silent second default, and whichever lost would be dead code.
    expect(sharedNominals.length, 'the shared sheet does not declare exactly one nominal').toBe(1);
    const box = sharedNominals[0];
    expect(box.props.get('--icon-glyph'), 'the default nominal moved off the large step')
      .toBe('var(--icon-size-lg)');
    // Same rule, so the box the nominal is declared on is the tap target itself.
    expect(box.props.get('width'), 'the nominal is not declared on the 2.25rem box').toBe('2.25rem');
    expect(box.props.get('height')).toBe('2.25rem');

    // The default is only worth anything while the glyph actually reads it.
    const svg = cssRules(css).find(r =>
      r.selector.split(',').some(s => s.trim() === '.icon-btn.row-icon svg'),
    );
    expect(svg, 'no .icon-btn.row-icon svg rule').toBeDefined();
    expect(svg!.props.get('width'), 'the glyph is not sized from the nominal').toBe('var(--icon-glyph)');
    expect(svg!.props.get('height')).toBe('var(--icon-glyph)');

    // And the trigger group's pair takes that default: nothing in their own
    // sheet retunes it, which is what keeps their rendered size untouched by
    // the queued override below (measured for real by
    // e2e/trigger-group-icon-optics.spec.ts).
    for (const name of ['trigger-group-delete', 'trigger-group-rename']) {
      for (const r of rulesTargeting(groupCss, name)) {
        expect(r.props.has('--icon-glyph'), `${name} picked up a local nominal`).toBe(false);
      }
    }
  });

  it('lets the queued trash pick a smaller step, on a selector that outranks the default', () => {
    expect(queuedNominals.length, 'the queued trash does not declare exactly one nominal').toBe(1);
    const queued = queuedNominals[0];

    // Specificity, not sheet order: the override must win wherever the two
    // sheets land relative to each other, so it carries more classes in its
    // subject than the default it is beating.
    const defaultClasses = subjectClasses(sharedNominals[0].selector, 'row-icon');
    const queuedClasses = subjectClasses(queued.selector, 'queued-message-remove');
    expect(queuedClasses.length, `"${queued.selector}" does not outrank "${sharedNominals[0].selector}"`)
      .toBeGreaterThan(defaultClasses.length);
    // Naming the box's own two classes is what makes it a specificity win over
    // that exact rule rather than a differently-shaped selector that misses.
    expect(queuedClasses).toEqual(expect.arrayContaining(['.icon-btn', '.row-icon', '.queued-message-remove']));

    // A step on the scale, and a smaller one: the point of the override.
    const value = queued.props.get('--icon-glyph')!;
    const token = /^var\((--icon-size-[\w-]+)\)$/.exec(value)?.[1];
    expect(token, `${value} is not an --icon-size-* step`).toBeDefined();
    const lg = steps.get('--icon-size-lg');
    expect(lg, '--icon-size-lg is not a rem step in base.css').toBeGreaterThan(0);
    expect(steps.get(token!), `${token} is not a rem step in base.css`).toBeGreaterThan(0);
    expect(steps.get(token!), `${token} is not smaller than --icon-size-lg`).toBeLessThan(lg!);

    // The three classes are only a specificity win while the button carries all
    // three; drop one in the TSX and the rule quietly stops matching.
    expect(chatExchange, 'the queued button no longer carries the classes the override names')
      .toContain('class="icon-btn row-icon queued-message-remove"');
  });

  it('shrinks the glyph only, leaving the 2.25rem tap target alone', () => {
    // The box was grown deliberately after this trash was reported unhittable
    // on mobile, so no rule aimed at this button may resize or re-pad it: the
    // glyph is the only thing the override is allowed to move.
    for (const r of queuedRules) {
      for (const prop of ['width', 'height', 'min-width', 'min-height', 'padding']) {
        expect(r.props.has(prop), `"${r.selector}" sets ${prop} on the tap target`).toBe(false);
      }
      for (const prop of r.props.keys()) {
        expect(prop.startsWith('padding-'), `"${r.selector}" sets ${prop} on the tap target`).toBe(false);
      }
    }
  });
});
