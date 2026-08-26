/**
 * The image popup is a viewer, so it may not reshape the image it shows.
 *
 * `border-radius` was the one that shipped. Nothing in CSS can tell whether an
 * image's edge belongs to the picture, and a radius cuts the ones that do: the
 * lucidos.dev og-card draws its own two-pixel rim, and `0.5rem` sliced each
 * corner off it. A browser tab rounds nothing, and the reader compares the two.
 *
 * `clip-path` is banned beside it, being what a future author reaches for
 * instead. So is `filter`, because a drop-shadow or a blur repaints the image
 * itself. A `box-shadow` is deliberately allowed: it paints outside the box and
 * leaves every pixel of the image alone.
 *
 * Scanned over every sheet rather than `components.css`, because a second sheet
 * re-adding the radius reads exactly like this rule never existing.
 */
import { describe, it, expect } from 'vitest';
import { cssRules, selectorList, styleSheetPaths } from './css-rule-helpers';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here: string = dirname(fileURLToPath(import.meta.url));
const STYLES: string = resolve(here, '..');
const BANNED = ['border-radius', 'clip-path', 'filter'];

/**
 * Does one selector-list member style the popup's own image?
 *
 * The subject is the image, not the slide, so `rulesTargeting` answers no: it
 * reports the element a CLASS sits on. The subject is the last compound, and a
 * tag name leads its compound, so `img.zoomed` has to count as much as a bare
 * `img`. Matching the member's tail against `img` alone would miss it, and a
 * guard a compound selector walks past is the shape of not having one.
 */
function stylesPopupImage(one: string): boolean {
  if (!/\bimage-popup\b/.test(one)) return false;
  const compounds = one.trim().split(/[\s>+~]+/);
  return /^img(?![\w-])/.test(compounds[compounds.length - 1] ?? '');
}

describe('the image popup does not reshape the image', () => {
  it('no sheet gives the popup image a radius, a clip or a filter', () => {
    const offenders: string[] = [];
    for (const path of styleSheetPaths(STYLES)) {
      const css: string = readFileSync(path, 'utf-8');
      for (const rule of cssRules(css)) {
        if (!selectorList(rule.selector).some(stylesPopupImage)) continue;
        for (const prop of BANNED) {
          const value = rule.props.get(prop);
          if (value !== undefined) offenders.push(`${path}: ${rule.selector} { ${prop}: ${value} }`);
        }
      }
    }
    expect(offenders, offenders.join('\n')).toEqual([]);
  });
});
