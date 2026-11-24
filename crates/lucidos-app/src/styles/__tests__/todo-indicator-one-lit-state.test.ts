import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { cssRules, styleSheetPaths, type CssRule } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const STYLES = resolve(here, '..');

const INDICATOR = '[data-role="todo-indicator"]';

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
    expect(found.map((f) => f.rule.body)).toEqual(['color: var(--accent)']);
  });
});
