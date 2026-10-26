/**
 * A menu opened from the composer paints above the scroll chevrons.
 *
 * The Apply split button's menu rises out of `.prompt-area` over the
 * transcript, right where `.scroll-to-bottom` floats. The chevron's z-index
 * outranks the composer's on a phone, so the open menu drew under it. The
 * composer lifts above the chevron while it holds an anchored overlay panel.
 *
 * Every rule that gives `.prompt-area` a z-index needs a lift of the same
 * selector, or its specificity beats the base lift on that layout.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const styles = (rel: string): string => readFileSync(resolve(here, '..', rel), 'utf-8');

// A modal opened from the composer (the camera) renders inside it. A lifted
// composer is a stacking context, which would trap the modal's z-index in it.
const LIFT = ':has([data-overlay-panel]):not(:has(.modal-overlay))';
const sheets = [styles('panels/content.css'), styles('mobile.css')];
const rules = sheets.flatMap(cssRules);

/** A `calc(var(--z-float) + n)` value, as its offset n. */
function floatOffset(value: string | undefined): number {
  const m = value?.match(/^calc\(var\(--z-float\) \+ (\d+)\)$/);
  expect(m, `expected calc(var(--z-float) + n), got ${value}`).not.toBeNull();
  return Number(m![1]);
}

describe('composer menu above the scroll chevron', () => {
  const chevron = cssRules(styles('chat/input-messages.css'))
    .find(r => r.selector === '.scroll-to-top, .scroll-to-bottom');
  const chevronOffset = floatOffset(chevron?.props.get('z-index'));

  it('lifts every z-indexed composer above the chevron while a menu is open', () => {
    const zIndexed = rules.filter(r =>
      /\.prompt-area$/.test(r.selector) && r.props.has('z-index') && !r.selector.includes(','));
    expect(zIndexed.length).toBeGreaterThan(0);
    for (const base of zIndexed) {
      const lift = rules.find(r => r.selector === `${base.selector}${LIFT}`);
      expect(lift, `no lift for \`${base.selector}\``).toBeDefined();
      expect(floatOffset(lift!.props.get('z-index'))).toBeGreaterThan(chevronOffset);
    }
  });
});
