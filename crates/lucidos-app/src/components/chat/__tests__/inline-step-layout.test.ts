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
  // children shrank proportionally. Detail uses flex: 1 1 0 so it absorbs
  // shrinkage first, leaving the description at its content width.
  it('step-detail must use flex 1 1 0 to absorb shrinkage', () => {
    const block = getBlock('.inline-step .step-detail');
    expect(block).toContain('flex: 1 1 0');
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

  // The shared .inline-step:hover rule lifts the description to --text-primary,
  // which would undo the muting on exactly the row being inspected.
  it('unfinished step stays muted on hover', () => {
    const block = getBlock('.inline-step.unfinished:hover .step-description');
    expect(block).toContain('color: var(--text-secondary)');
  });
});
