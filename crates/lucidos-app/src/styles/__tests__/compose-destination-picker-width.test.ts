/**
 * The compose destination dropdown must resolve to the same width as the
 * prompt box below it, on desktop and at phone width. `.input-toggles-wrapper`
 * sizes each child to its own content (`align-items: flex-start`). So
 * `.compose-destination-row` needs its own stretch to reach the wrapper's
 * full width, and the picker inside it needs to grow to fill the row. Drop
 * either declaration and the dropdown shrinks back to its label's width,
 * while `.prompt-box` stays full width: the regression this guards.
 */
import { describe, it, expect } from 'vitest';
import postcss, { type Rule } from 'postcss';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const INPUT_CSS = resolve(here, '../chat/input-messages.css');

/** Declarations of one rule, by selector, in the given stylesheet. */
function declarationsOf(cssPath: string, selector: string): Record<string, string> {
  const root = postcss.parse(readFileSync(cssPath, 'utf8'));
  const out: Record<string, string> = {};
  root.walkRules((rule: Rule) => {
    if (rule.selector.trim() !== selector) return;
    rule.walkDecls((d) => {
      out[d.prop] = d.value.trim();
    });
  });
  return out;
}

describe('the compose destination picker matches the prompt box width', () => {
  it('stretches the row to the wrapper\'s full width', () => {
    const row = declarationsOf(INPUT_CSS, '.compose-destination-row');
    expect(row['width']).toBe('100%');
    expect(row['align-self']).toBe('stretch');
  });

  it('grows the picker to fill the row instead of its label width', () => {
    const picker = declarationsOf(INPUT_CSS, '.compose-destination-row .compose-destination-picker');
    expect(picker['flex']).toBe('1 1 auto');
  });

  it('stretches the trigger button to fill the picker', () => {
    const trigger = declarationsOf(INPUT_CSS, '.compose-destination-row .compose-destination-picker .dropdown-trigger');
    expect(trigger['width']).toBe('100%');
  });

  it('leaves the coding-agent chip beside it at its own content width', () => {
    // So the picker, not the chip, is what absorbs the row's stretch.
    const chip = declarationsOf(INPUT_CSS, '.compose-coding-agent-chip');
    expect(chip['flex-shrink']).toBe('0');
  });
});
