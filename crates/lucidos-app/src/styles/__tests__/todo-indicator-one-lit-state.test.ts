/**
 * The todo indicator paints ONE state: accent while an item is in progress.
 *
 * Accent means something is live right now, which is the composer row's whole
 * language. The rest of what the list says reaches the reader in words: the
 * tooltip, the aria-label, the menu row, and the panel.
 *
 * Two more states used to paint, and this scan is what keeps them off. A
 * `waiting` item pulsed gray. That repeated the waiting indicator beside it,
 * which renders in accent for exactly the live event wait the item is parked
 * on. An `abandoned` item dimmed to 0.6, which reads as a disabled button.
 * Either is easy to re-add as a tidy-up, and neither would fail a build. `tsc`
 * never reads CSS, and `vite build` fails only on syntax.
 *
 * The scan covers every shipping stylesheet, not todo-list.css alone. A second
 * rule elsewhere in the bundle paints just as well.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { cssRules, styleSheetPaths, type CssRule } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
/** `crates/lucidos-app/src/styles/`, from `src/styles/__tests__/`. */
const STYLES = resolve(here, '..');

const INDICATOR = '[data-role="todo-indicator"]';

/** Every shipping rule whose selector names the indicator, tagged with the file
 *  it came from so a failure says where to look. Read once at module load: both
 *  tests ask the same question of the same tree. */
const found: { file: string; rule: CssRule }[] = styleSheetPaths(STYLES)
  .flatMap((file: string) =>
    cssRules(readFileSync(file, 'utf8'))
      .filter((rule) => rule.selector.includes(INDICATOR))
      .map((rule) => ({ file, rule })),
  );

describe('the todo indicator has one lit state', () => {
  it('is painted by exactly one rule, and that rule is the in-progress one', () => {
    const where = found.map((f) => `  ${f.rule.selector}  (${f.file})`).join('\n');
    expect(found.length, `exactly one rule may paint ${INDICATOR}:\n${where}`).toBe(1);
    expect(found[0].rule.selector).toContain('[data-state="in-progress"]');
  });

  it('spends the accent and nothing else, so no second channel creeps back', () => {
    // The whole declaration list, not a named property. The abandoned dim was
    // an `opacity`, the waiting pulse an `animation`. A guard on `color` alone
    // would watch the one channel neither of them used.
    expect(found.map((f) => f.rule.body)).toEqual(['color: var(--accent)']);
  });
});
