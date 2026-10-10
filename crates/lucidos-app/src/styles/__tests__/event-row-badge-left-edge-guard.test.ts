/**
 * A badge header is an event row whose subject is just the event-type chip
 * (`.event-name`, `components/chat/EventRow.tsx`): "Child thread returned",
 * "Child thread stopped", a trigger fire, a boundary event. The chip carries
 * its own padding and a 1px border. Without correction its label sits inset
 * from the plain text below it: the facts row and the fold toggle, both
 * flush against the card's left edge.
 *
 * `.event-row-subject > .event-name:first-child` pulls the chip left by
 * exactly that padding and border, so the badge's text and the rows below it
 * share one left edge (`crates/lucidos-app/src/styles/chat/event-rows.css`).
 * This test pins the two rules staying in sync: if `.event-name`'s own
 * horizontal padding changes, the alignment rule must still cancel it out.
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
const sheetPath: string = resolve(here, '../chat/event-rows.css');
const css: string = readFileSync(sheetPath, 'utf-8');
const icons: string = readFileSync(resolve(here, '../../components/shared/icons.tsx'), 'utf-8');
const rules = cssRules(css);

describe('the event-row badge header lines up with the rows below it', () => {
  it('declares its chip padding as a custom property', () => {
    const chip = rules.find(r => r.selector === '.event-name');
    expect(chip?.props.get('--event-name-h-pad')).toBe('0.375rem');
    expect(chip?.props.get('padding')).toBe('0.0625rem var(--event-name-h-pad)');
  });

  it('pulls a leading chip left by exactly that padding plus its border', () => {
    const aligned = rules.find(r => r.selector === '.event-row-subject > .event-name:first-child');
    expect(aligned?.props.get('margin-left')).toBe('calc(-1px - var(--event-name-h-pad))');
  });

  /** The card adds the pull back on its left inset, so the badge's box sits as
   *  far from the left edge as from the top. */
  it('insets the card so the pulled badge sits as far from the left as the top', () => {
    const card = rules.find(r => r.selector === '.event-row');
    expect(card?.props.get('--event-row-inset')).toBe('0.625rem');
    expect(card?.props.get('padding')).toBe(
      'var(--event-row-inset) var(--space-md) var(--event-row-inset) calc(var(--event-row-inset) + 0.375rem + 1px)',
    );
    const chip = rules.find(r => r.selector === '.event-name');
    expect(chip?.props.get('--event-name-h-pad')).toBe('0.375rem');
  });

  it('never re-adds a left margin on the chip itself, which would fight the pull', () => {
    const chip = rules.find(r => r.selector === '.event-name');
    expect(chip?.props.get('margin-left')).toBeUndefined();
    expect(chip?.props.get('margin')).toBeUndefined();
  });

  /** The chevron's ink starts a third of an em into its box. Without a pull,
   *  the drawn `>` sits right of the text edge above it. */
  it('pulls the fold chevron left by the icon\'s own side bearing', () => {
    const chevron = rules.find(r => r.selector === '.event-row-fold-chevron');
    expect(chevron?.props.get('margin-left')).toBe('calc(-8em / 24)');
    // Turned down, the ink starts 5 units in, so it shifts back by 3.
    const open = rules.find(r => r.selector === '.event-row-fold-toggle[aria-expanded="true"] > .event-row-fold-chevron');
    expect(open?.props.get('transform')).toBe('translateX(calc(3em / 24)) rotate(90deg)');
    const right = icons.match(/function ChevronRightIcon[\s\S]*?<\/svg>/)?.[0] ?? '';
    expect(right).toContain('viewBox="0 0 24 24"');
    expect(right).toContain('stroke-width="2"');
    expect(right).toContain('points="9 6 15 12 9 18"');
  });
});
