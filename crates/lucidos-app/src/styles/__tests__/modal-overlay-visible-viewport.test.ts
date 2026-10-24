/**
 * Every modal centres in what is on screen, not behind the iOS keyboard.
 *
 * iOS never shrinks the layout viewport for the keyboard, so a scrim bounded
 * by it puts a dialog's buttons under the keys. Scanned rather than measured:
 * only a real iPhone raises that keyboard.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, decl, rulesTargeting, styleSheetPaths } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const base: string = readFileSync(resolve(here, '../global/modal-overlay.css'), 'utf-8');
// Components keep stylesheets beside them, so scan all of `src/`.
const everySheet: string[] = styleSheetPaths(resolve(here, '../..'))
  .map((p: string) => readFileSync(p, 'utf-8'));

function rulesSetting(className: string, props: string[]) {
  return everySheet
    .flatMap((sheet: string) => rulesTargeting(sheet, className))
    .filter((rule) => props.some((p) => rule.props.has(p)));
}

describe('the modal backdrop', () => {
  it('is bounded by the visible viewport', () => {
    const overlay = block(base, '.modal-overlay {');
    expect(decl(overlay, 'height')).toBe('var(--app-height, 100dvh)');
    expect(decl(overlay, 'bottom'), 'a bottom edge would stretch it back to the layout viewport')
      .toBe('auto');
  });

  // The bound belongs to the base class. A per-modal copy means the next
  // modal is built without it.
  it('is not re-bounded by one modal alone', () => {
    const copies = rulesSetting('modal-overlay', ['height', 'bottom'])
      .filter((rule) => rule.selector !== '.modal-overlay');
    expect(copies.map((r) => r.selector)).toEqual([]);
  });

  // The height is fixed, so a backdrop that moves its top edge runs off the
  // bottom of the screen by that much. Room for a header goes in padding.
  it('keeps its top edge where file search clears the mobile header', () => {
    expect(rulesSetting('file-search-overlay', ['top', 'inset']).map((r) => r.selector))
      .toEqual([]);
  });
});
