/**
 * A settings control stays on the right when it wraps under its label.
 *
 * The row wraps once the control no longer fits beside the label. With
 * `justify-content: space-between`, a control alone on its line sat at the
 * LEFT edge: Appearance → Motion on a phone put its segmented control under
 * "Motion", flush left, while every other row's control sat on the right.
 *
 * Packing to the end fixes that and changes nothing on a shared line, because
 * the label grows to fill it. So both halves are pinned here.
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

  // A fixed floor reserved as much room for "Mode" as for a long label. So a
  // short label dropped its control onto a new line with room to spare.
  it('bases the label on its own text, so a short label keeps its control', () => {
    const flex = rule('.settings-row > .settings-row-label').props.get('flex');
    expect(flex?.split(/\s+/)[2]).toBe('auto');
  });
});
