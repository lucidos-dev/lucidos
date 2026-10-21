/**
 * The dashboard sheet rises from the bottom edge, and under reduced motion it
 * only fades. Source scans of panels/dashboard.css, like the rail's chrome test.
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
  it('rests below the window and rises on a scaled transform transition', () => {
    const closed = rule('.dashboard-sheet-panel').props;
    expect(closed.get('transform')).toBe('translateY(100%)');
    expect(closed.get('transition')).toContain('transform var(--duration-slow)');
    const open = rule('.dashboard-sheet.is-open .dashboard-sheet-panel').props;
    expect(open.get('transform')).toBe('none');
  });

  it('fades the dim alongside, on the same scaled duration', () => {
    expect(rule('.dashboard-sheet-dim').props.get('transition')).toContain('opacity var(--duration-slow)');
  });

  it('only fades under reduced motion: no transform moves', () => {
    const reduced = rule(':root[data-motion="reduce"] .dashboard-sheet-panel').props;
    expect(reduced.get('transform')).toBe('none');
    expect(reduced.get('transition')).not.toContain('transform');
    expect(reduced.get('transition')).toContain('opacity');
    const reducedOpen = rule(':root[data-motion="reduce"] .dashboard-sheet.is-open .dashboard-sheet-panel').props;
    expect(reducedOpen.get('opacity')).toBe('1');
  });

  it('leaves the title row to stay clear: the dims never reach into it', () => {
    expect(rule('.dashboard-sheet-dim-top').props.get('height')).toBe('var(--sheet-row-top)');
    expect(rule('.dashboard-sheet-dim-gap').props.get('top')).toBe('var(--sheet-row-bottom)');
    expect(rule('.dashboard-sheet-panel').props.get('top')).toBe('calc(var(--sheet-row-bottom) + var(--sheet-gap))');
  });
});
