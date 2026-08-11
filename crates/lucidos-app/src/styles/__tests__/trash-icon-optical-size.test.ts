/**
 * The trash's optical correction stays internally consistent.
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
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { cssRules } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(resolve(here, '../global/host-components.css'), 'utf8');
const icons = readFileSync(resolve(here, '../../components/shared/icons.tsx'), 'utf8');

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
        .toMatch(/calc\(var\(--icon-size-lg\) \* var\(--pencil-ink\) \/ var\(--trash-ink\)\)/);
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
