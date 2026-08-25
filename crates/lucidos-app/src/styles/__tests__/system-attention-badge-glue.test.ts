/**
 * The *System attention badge*'s dot is glued to the label it marks.
 *
 * A dot is an atomic inline, and line breaking allows a break in front of one.
 * A squeezed label therefore drops its trailing mark onto a line of its own.
 * Under the text and next to nothing, it reads as a stray glyph. The fix is the
 * U+2060 word joiner, the same one `.explainer-slot` carries, and this pins
 * that the slot keeps it.
 *
 * Two things break the joiner silently, so both are checked. An `inline-block`
 * or `inline-flex` wrapper is an atomic inline itself and hides it. And a text
 * node in the markup would put it in `textContent`, where an exact-text
 * assertion trips over an invisible character.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { block, decl } from './css-rule-helpers';

const CSS = readFileSync(
  fileURLToPath(new URL('../global/host-components.css', import.meta.url)), 'utf8',
);
const MARKUP = readFileSync(
  fileURLToPath(new URL('../../components/shared/SystemAttentionBadge.tsx', import.meta.url)), 'utf8',
);

describe('the What\'s New badge dot', () => {
  it('wears the accent token, never a colour of its own', () => {
    expect(decl(block(CSS, '.system-attention-badge {'), 'background')).toBe('var(--accent)');
  });

  it('takes its corner placement from `.badge`, never a second copy', () => {
    // `.badge` already carries the absolute corner AND, through
    // `.app-header .badge`, the bar's repaint and the ring that stops a mark
    // fusing with its glyph. Restating the geometry here would opt out of both.
    const corner = block(CSS, '.system-attention-badge-corner {');
    expect(decl(corner, 'position')).toBe(null);
    expect(decl(corner, 'background')).toBe(null);
    // What IS its own: an empty box stays round, where `.badge` pads and
    // line-boxes for a glyph.
    expect(decl(corner, 'padding')).toBe('0');
    expect(decl(corner, 'border-radius')).toBe('50%');
  });

  it('outranks `.badge`, which a later sheet would otherwise win on order', () => {
    // `main.tsx` imports `header.css`, where `.badge` lives, AFTER this sheet.
    // A single-class modifier ties on specificity and loses, so the dot renders
    // at the counted badge's size as an empty pill.
    expect(CSS).toMatch(/\n\.badge\.system-attention-badge-corner \{/);
  });
});

describe('the inline slot', () => {
  const slot = block(CSS, '.system-attention-badge-slot {');

  it('is a plain inline box, so the joiner is not hidden by an atomic one', () => {
    expect(decl(slot, 'display')).toBe('inline');
    expect(decl(slot, 'line-height')).toBe('0');
  });

  it('leaves the explainer\'s own slot rule alone', () => {
    // The joiner is a pattern to apply, not a class to share: two components
    // that may drift apart. `explainer.test.ts` pins its copy by selector, so
    // folding the two into one selector list breaks that guard.
    expect(CSS).toMatch(/\n\.explainer-slot \{/);
  });

  it('forbids a break either side of the dot', () => {
    expect(decl(block(CSS, '.system-attention-badge-slot::before {'), 'content')).toBe("'\\2060'");
  });

  it('keeps the joiner out of the markup, so it stays out of textContent', () => {
    expect(MARKUP).not.toContain('⁠');
  });
});
