/**
 * A row of buttons must stay BOUND BY ITS CONTAINER: buttons that do not fit
 * stack onto a second row, and a button whose label is wider than the whole row
 * ellipsizes rather than being sliced by an ancestor's `overflow: hidden`.
 *
 * The regression this pins shipped on the "New version available" toast at a
 * 320px viewport (an iPhone in Display Zoom): "Later" + "Switch to new version"
 * were about 30px wider than the toast's content box, the non-wrapping
 * `.toast-actions` row overflowed, and `.toast { overflow: hidden }` cut the
 * last letter off "Switch to new version".
 *
 * A source scan rather than a browser test, for the same reason the transcript
 * fades are scanned: the regression is about which declarations are written,
 * and the three Playwright projects run at 1280 / 375 / 390 px, so none of them
 * is narrow enough for the overflow to appear. Scanning also pins the part a
 * rendered assertion could not see, namely that the toast USES the shared
 * primitive rather than re-declaring a local copy of it, which is what keeps
 * the fix general (it ships to app iframes through
 * `/api/v1/sdk-iframe.css`) instead of toast-specific.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, decl } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const sharedCss = readFileSync(resolve(here, '../global/shared-components.css'), 'utf-8');
const componentsCss = readFileSync(resolve(here, '../components.css'), 'utf-8');
const toastTsx = readFileSync(resolve(here, '../../components/shared/Toast.tsx'), 'utf-8');

describe('.button-group keeps a row of buttons inside its container', () => {
  it('wraps, so buttons stack instead of overflowing', () => {
    const rule = block(sharedCss, '.button-group {');
    expect(decl(rule, 'display')).toBe('flex');
    expect(
      decl(rule, 'flex-wrap'),
      'without flex-wrap the row overflows its container instead of stacking',
    ).toBe('wrap');
  });

  it('leaves alignment to the consumer', () => {
    // A toast right-aligns its actions; a form may not. Baking one in here
    // would make every adopter fight it.
    expect(decl(block(sharedCss, '.button-group {'), 'justify-content')).toBeNull();
  });

  it('bounds each button so an over-long label ellipsizes', () => {
    const rule = block(sharedCss, '.button-group > .action-btn {');
    // `.action-btn` is `white-space: nowrap`, so all three are load-bearing:
    // without the max-width the button is never narrower than its label,
    // without overflow:hidden the label paints past the button, and without
    // text-overflow the truncation has no ellipsis.
    expect(decl(rule, 'max-width')).toBe('100%');
    expect(decl(rule, 'overflow')).toBe('hidden');
    expect(decl(rule, 'text-overflow')).toBe('ellipsis');
    // A min always beats a max in CSS, so `.action-btn`'s own 3.5rem floor
    // survives the bound. Overriding it here would shrink ordinary buttons.
    expect(
      decl(rule, 'min-width'),
      'a min-width override here would drop .action-btn\'s 3.5rem floor',
    ).toBeNull();
  });

  it('is the primitive the toast uses, not a copy the toast keeps', () => {
    expect(
      toastTsx,
      'the toast action row must opt into the shared primitive',
    ).toContain('class="toast-actions button-group"');
    const rule = block(componentsCss, '.toast-actions {');
    expect(decl(rule, 'justify-content'), 'the toast owns only its alignment').toBe('flex-end');
    for (const prop of ['display', 'flex-wrap', 'gap']) {
      expect(
        decl(rule, prop),
        `.toast-actions re-declares ${prop}; that belongs to .button-group`,
      ).toBeNull();
    }
  });
});
