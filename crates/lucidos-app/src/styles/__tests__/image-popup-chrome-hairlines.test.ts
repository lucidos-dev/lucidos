/**
 * The image popup outlines its chrome in white over arbitrary photo content.
 * Those rings are the one place a hairline has to survive a curve, and two
 * properties keep them even. Both are invisible in a review diff.
 *
 * A ring needs two DEVICE pixels of width. At one, the straight runs land on a
 * whole row and paint solid. The arcs spread over two rows and paint at part
 * coverage, so the ring thins and brightens as it turns. Raising the alpha
 * gives a brighter uneven ring, so width is what carries it.
 *
 * The ring's BOX has to land on whole CSS pixels too, or its ends disagree.
 * Root font size is always an even whole number of px, so a 0.5rem multiple is
 * always whole. A padding may be fractional alone if the pair sums to one, so
 * this test adds the boxes up rather than scanning each declaration.
 *
 * Scanned, not measured in a browser: the failure is a property of the rule at
 * every scale, and it needs a 1x display no CI machine offers.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, decl, rulesTargeting } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const componentsCss = readFileSync(resolve(here, '../components.css'), 'utf-8');

/** Every root font size the UI scale can produce, in px. `UI_SCALE_MIN` to
 *  `UI_SCALE_MAX` in `UI_SCALE_STEP` steps (packages/lucidos-sdk/src/appearance.ts),
 *  against the 16px browser default. */
const ROOT_SIZES: number[] = [];
for (let scale = 75; scale <= 200; scale += 12.5) ROOT_SIZES.push((16 * scale) / 100);

/** The classes whose ring the popup draws in white over the image. */
const RINGED = ['image-popup-close', 'image-popup-nav', 'image-popup-zoom', 'floating-mobile-close'];

/** The widest ring, which is what the boxes have to stay whole against. A
 *  narrower one only subtracts a whole px from a whole total. */
const RING_PX = 2;

/**
 * A length in px at the given root font size.
 *
 * Handles the three shapes these rules use: a rem literal, a px literal, and a
 * `calc()` summing them. Anything else throws rather than answering zero: a
 * value this test cannot read is one it cannot vouch for.
 */
function px(value: string, root: number): number {
  const calc = value.match(/^calc\((.+)\)$/);
  if (calc) return calc[1].split('+').reduce((sum, term) => sum + px(term.trim(), root), 0);
  if (value === 'var(--image-popup-ring)') return RING_PX;
  const rem = value.match(/^(-?[\d.]+)rem$/);
  if (rem) return Number(rem[1]) * root;
  const raw = value.match(/^(-?[\d.]+)px$/);
  if (raw) return Number(raw[1]);
  throw new Error(`cannot resolve "${value}" to px`);
}

const closeRule = block(componentsCss, '.image-popup-close {');
const zoomRule = block(componentsCss, '.image-popup-zoom {');
const zoomBtnRule = block(componentsCss, '.image-popup-zoom-btn {');
const zoomLevelRule = block(componentsCss, '.image-popup-zoom-level {');

/** Outer size of the zoom pill, a shrink-to-fit row: its own padding and
 *  borders, its three buttons at their floor widths, and the two gaps. */
function pillBox(root: number): { width: number; height: number } {
  const pad = px(decl(zoomRule, 'padding')!, root);
  const gap = px(decl(zoomRule, 'gap')!, root);
  const ring = px(decl(zoomRule, 'border')!.split(' ')[0], root);
  const btn = px(decl(zoomBtnRule, 'min-width')!, root);
  const level = px(decl(zoomLevelRule, 'min-width')!, root);
  const btnHeight = px(decl(zoomBtnRule, 'height')!, root);
  return {
    width: 2 * ring + 2 * pad + 2 * gap + 2 * btn + level,
    height: 2 * ring + 2 * pad + btnHeight,
  };
}

describe('the image popup draws even hairlines', () => {
  it('widens the ring to two device pixels below 2dppx', () => {
    expect(decl(block(componentsCss, '.image-popup {'), '--image-popup-ring')).toBe(`${RING_PX}px`);

    // Both queries. Safari took `min-resolution` in dppx late, and the
    // `-webkit-` alias is what covers the versions before it.
    const prelude = '@media (min-resolution: 2dppx), (-webkit-min-device-pixel-ratio: 2)';
    expect(componentsCss, 'the retina arm needs both spellings').toContain(prelude);
    expect(decl(block(componentsCss, prelude), '--image-popup-ring')).toBe('1px');
  });

  it('draws every ringed control at that width, never a literal one', () => {
    const literal = RINGED.flatMap((cls) =>
      rulesTargeting(componentsCss, cls)
        .filter((rule) => rule.props.has('border'))
        .filter((rule) => !rule.props.get('border')!.startsWith('var(--image-popup-ring)'))
        .map((rule) => `${rule.atRules} ${rule.selector} { border: ${rule.props.get('border')} }`),
    );

    expect(literal, 'a fixed border width leaves this ring ragged on a 1x display').toEqual([]);
  });

  it('keeps the pill a true stadium by deriving its radius from its own height', () => {
    const radius = decl(zoomRule, 'border-radius')!;
    for (const root of ROOT_SIZES) {
      expect(px(radius, root), `radius at root ${root}px`).toBe(pillBox(root).height / 2);
    }
  });

  it('lands both boxes on whole pixels at every UI scale', () => {
    const fractional: string[] = [];
    for (const root of ROOT_SIZES) {
      const close = px(decl(closeRule, 'width')!, root);
      expect(px(decl(closeRule, 'height')!, root), 'the close button is a circle').toBe(close);
      const pill = pillBox(root);
      const boxes: [string, number][] = [
        ['close', close],
        ['pill width', pill.width],
        ['pill height', pill.height],
      ];
      for (const [what, value] of boxes) {
        if (!Number.isInteger(value)) fractional.push(`root ${root}px: ${what} = ${value}px`);
      }
    }

    expect(fractional, 'a hairline around a fractional box paints unevenly').toEqual([]);
  });

  /** The close button borrows `.icon-btn`, whose padding box is what made it
   *  32.25px wide at 137.5%. An explicit box replaced it, so the padding has to
   *  stay out of the sum. */
  it('sizes the close button itself rather than inheriting .icon-btn padding', () => {
    expect(decl(closeRule, 'padding')).toBe('0');
    expect(decl(closeRule, 'width')).not.toBeNull();
    expect(decl(closeRule, 'height')).not.toBeNull();
  });
});
