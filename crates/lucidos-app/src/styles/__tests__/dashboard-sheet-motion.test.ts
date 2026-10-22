/**
 * The dashboard sheet fades in place, on a scaled duration, so reduced motion
 * needs no rule of its own. Source scans of panels/dashboard.css, like the
 * rail's chrome test.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules, type CssRule } from './css-rule-helpers';

const stylesDir: string = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const css: string = readFileSync(resolve(stylesDir, 'panels/dashboard.css'), 'utf-8');

function rule(selector: string): CssRule {
  const found = cssRules(css).filter((r) => r.selector === selector);
  expect(found.length, `expected exactly one \`${selector}\` rule`).toBe(1);
  return found[0];
}

describe('the dashboard sheet motion', () => {
  it('fades in place on a scaled opacity transition, and never slides', () => {
    const closed = rule('.dashboard-sheet-panel').props;
    expect(closed.get('opacity')).toBe('0');
    expect(closed.get('visibility')).toBe('hidden');
    expect(closed.get('transition')).toContain('opacity var(--duration-normal)');
    expect(closed.has('transform')).toBe(false);
    const open = rule('.dashboard-sheet.is-open .dashboard-sheet-panel').props;
    expect(open.get('opacity')).toBe('1');
    expect(open.get('visibility')).toBe('visible');
  });

  it('meets the app header with no gap, line or corner', () => {
    const panel = rule('.dashboard-sheet-panel').props;
    expect(panel.get('top')).toBe('var(--app-header-bottom)');
    expect(panel.has('border-top')).toBe(false);
    expect(panel.has('border-radius')).toBe(false);
  });
});
