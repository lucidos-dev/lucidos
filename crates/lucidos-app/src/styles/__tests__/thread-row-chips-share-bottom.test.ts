import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const drawerCss = readFileSync(resolve(here, '../drawer.css'), 'utf-8');

function rule(selector: string) {
  const found = cssRules(drawerCss).find(r =>
    r.selector.split(',').some(s => s.trim() === selector));
  expect(found, `no rule for ${selector}`).toBeDefined();
  return found!;
}

describe('thread row meta line', () => {
  it('aligns the date and the chips on their last baseline', () => {
    expect(rule('.thread-row-meta').props.get('align-items')).toBe('last baseline');
  });

  it('puts the sub-thread controls on one text baseline', () => {
    expect(rule('.thread-row-family-line').props.get('align-items')).toBe('last baseline');
  });

  it('lets the sub-thread link follow the line\'s baseline', () => {
    expect(rule('.thread-row-family-line > .family-disclosure').props.get('align-self')).toBe('auto');
  });

  it('centres the archived switch on its label', () => {
    expect(rule('.archived-reveal').props.get('display')).toBe('inline-block');
    expect(rule('.archived-reveal-toggle').props.get('vertical-align')).toBe('middle');
  });

  it('wraps the sub-thread line rather than overflowing the row', () => {
    expect(rule('.thread-row-family-line').props.get('flex-wrap')).toBe('wrap');
  });

  it('centres the skeleton chip, which has no baseline', () => {
    expect(rule('.thread-row-meta > .sk-bar').props.get('align-self')).toBe('center');
  });
});
