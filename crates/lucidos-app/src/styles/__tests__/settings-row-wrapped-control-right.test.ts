import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const settingsCss = readFileSync(resolve(here, '../settings/base.css'), 'utf-8');

function rule(selector: string) {
  const found = cssRules(settingsCss).find(r => r.selector === selector);
  expect(found, `no rule for ${selector}`).toBeDefined();
  return found!;
}

describe('settings row alignment', () => {
  it('packs the row to the end, so a wrapped control stays right', () => {
    expect(rule('.settings-row').props.get('justify-content')).toBe('flex-end');
  });

  it('grows the label, so a shared line still puts the control right', () => {
    const flex = rule('.settings-row > .settings-row-label').props.get('flex');
    expect(flex?.split(/\s+/)[0]).toBe('1');
  });

  it('bases the label on its own text, so a short label keeps its control', () => {
    const flex = rule('.settings-row > .settings-row-label').props.get('flex');
    expect(flex?.split(/\s+/)[2]).toBe('auto');
  });

  it('centres a lone option button on its label', () => {
    const row = '.settings-row:has(> .settings-row-label + .settings-option)';
    expect(rule(row).props.get('align-items')).toBe('center');
    expect(rule(`${row} > .settings-row-label`).props.get('padding-top')).toBe('0');
  });
});
