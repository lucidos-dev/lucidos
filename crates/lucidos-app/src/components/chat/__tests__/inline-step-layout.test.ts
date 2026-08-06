import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));

/**
 * Regression test: inline steps must render checkmark and description on the
 * same line, with ellipsis truncation for long descriptions.
 *
 * Previous bug: `flex-wrap: wrap` on `.inline-step` caused the description to
 * wrap below the checkmark icon when text was long. Combined with
 * `white-space: nowrap` but no overflow handling, text was clipped without
 * ellipsis.
 */
describe('inline step layout (CSS regression)', () => {
  const css = readFileSync(resolve(here, '../../../styles/steps.css'), 'utf-8');

  // Extract the CSS block for a selector
  function getBlock(selector: string): string {
    const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    const re = new RegExp(`${escaped}\\s*\\{([^}]*)\\}`, 'g');
    return [...css.matchAll(re)].map(m => m[1]).join('\n');
  }

  it('inline-step must not flex-wrap (checkmark and text stay on same line)', () => {
    const block = getBlock('.inline-step');
    // flex-wrap: wrap causes the description to drop below the icon
    expect(block).not.toContain('flex-wrap: wrap');
  });

  it('step-description must truncate with ellipsis', () => {
    const block = getBlock('.inline-step .step-description');
    expect(block).toContain('overflow: hidden');
    expect(block).toContain('text-overflow: ellipsis');
  });

  // Regression: short description (e.g. "Memory searched") was clipped to
  // "Memory searc…" when the sibling detail was long, because both flex
  // children shrank proportionally. Detail uses flex: 1 1 0 so it takes the
  // leftover space, leaving the description at its content width.
  it('step-detail must use flex 1 1 0 to absorb shrinkage', () => {
    const block = getBlock('.inline-step .step-detail');
    expect(block).toContain('flex: 1 1 0');
  });

  // Regression (the event-wait park, which has a long description AND a long
  // detail by construction): a flex basis of 0 also means a scaled SHRINK
  // factor of 0, so a description wider than the row absorbed all of the
  // shrinkage and left the detail at exactly 0px. The detail wrapped and broke
  // words anywhere, so 0px meant one character per line: a 935px-tall step row
  // measured at a 330px phone width. Both halves of the fix are load-bearing,
  // so both are pinned: the row is single-line, and the detail keeps a floor.
  it('step-detail must be single-line, and must never be crushed to nothing', () => {
    const block = getBlock('.inline-step .step-detail');
    expect(block).toContain('white-space: nowrap');
    expect(block).toContain('text-overflow: ellipsis');
    expect(block).toContain('overflow: hidden');
    // Declaration-anchored: the block's own comment names the property it
    // dropped, and a substring check would read that as the property itself.
    expect(block).not.toMatch(/^\s*word-break:/m);
    expect(block).not.toMatch(/^\s*min-width:\s*0/m);
    expect(block).toMatch(/^\s*min-width:\s*min\(/m);
  });

  // Regression: the context counter wrapped to three lines ("178k /", "1000k",
  // "(18%)") on a phone, tripling the height of every step row. It is a
  // fixed-width fact, so it neither wraps nor shrinks; the description
  // ellipsizes to make room.
  it('step-context must not wrap or shrink', () => {
    const block = getBlock('.inline-step .step-context');
    expect(block).toContain('white-space: nowrap');
    expect(block).toContain('flex-shrink: 0');
  });

  // A step killed mid-execution (the turn died before the tool reported a
  // result) reads as "did not finish": muted and struck. Deliberately NOT the
  // red .error treatment, which asserts the tool ran and returned a failure.
  it('unfinished step is muted and struck, never the red failure treatment', () => {
    const description = getBlock('.inline-step.unfinished .step-description');
    expect(description).toContain('text-decoration: line-through');
    expect(description).toContain('color: var(--text-muted)');

    const icon = getBlock('.inline-step.unfinished .step-icon');
    expect(icon).toContain('color: var(--text-muted)');
    expect(icon).not.toContain('--accent-red');
  });

  // The shared .step-main:hover rule lifts the description to --text-primary,
  // which would undo the muting on exactly the row being inspected.
  it('unfinished step stays muted on hover', () => {
    const block = getBlock('.inline-step.unfinished .step-main:hover .step-description');
    expect(block).toContain('color: var(--text-secondary)');
  });

  // The row folds thinking and the call it produced into one row with TWO click
  // targets, so it is a <div> around two <button>s (a button may not contain
  // another interactive element). The main target must therefore carry the flex
  // layout the row used to own, or the icon/description/detail stack vertically
  // inside it.
  it('step-main is a flex row that absorbs the width the counter will not', () => {
    const block = getBlock('.inline-step .step-main');
    expect(block).toContain('display: flex');
    expect(block).toContain('align-items: baseline');
    expect(block).toContain('flex: 1 1 auto');
    expect(block).toContain('min-width: 0');
  });

  // Both targets sit in a transcript row and must read as text rather than as
  // controls, so neither may keep the native button chrome. Each owns its own
  // reset: sharing one rule between them put the counter's colour, size and
  // margin at the mercy of which selector out-ranked which.
  it('both click targets strip the native button chrome, each in its own rule', () => {
    for (const selector of ['.inline-step .step-main', '.inline-step .step-context']) {
      const block = getBlock(selector);
      expect(block).toContain('background: none');
      expect(block).toContain('border: none');
      expect(block).toContain('padding: 0');
    }
  });

  // A legacy row's counter has no snapshot to open, so it renders as a bare
  // <span>; a pointer cursor there is the same lie as a button that opens
  // nothing. So the pointer is element-qualified, and that qualified rule holds
  // NOTHING else: at (0,2,1) it out-ranks the counter's own (0,2,0) rule, and
  // anything else parked in it would silently override the counter's styling.
  it('only the button counter is interactive, and the qualified rule carries nothing else', () => {
    const interactive = getBlock('.inline-step button.step-context');
    expect(interactive).toContain('cursor: pointer');
    for (const stolen of ['color:', 'font:', 'font-size:', 'margin', 'background', 'padding']) {
      expect(interactive).not.toContain(stolen);
    }
    // No class-only rule may hand the pointer to the inert span.
    expect(getBlock('.inline-step .step-context')).not.toContain('cursor: pointer');
  });
});
