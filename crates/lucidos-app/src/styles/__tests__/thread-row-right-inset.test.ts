import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

import { cssRules, selectorList } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const CSS = readFileSync(resolve(here, '../drawer.css'), 'utf8');
const RULES = cssRules(CSS);

function ruleFor(sel: string, atRules: string, prop?: string) {
  const found = RULES.filter(r => r.selector === sel && r.atRules === atRules
    && (!prop || r.props.has(prop)));
  expect(found, `no rule "${sel}" under "${atRules || 'top level'}"`).toHaveLength(1);
  return found[0];
}

const MOBILE = '@media (--phone-layout)';
const VAR = '--thread-row-pad-right';

describe('the thread row right inset', () => {
  it('is declared once on the drawer, as a var the row pads by', () => {
    expect(ruleFor('.thread-drawer', '', VAR).props.get(VAR)).toBe('0.5rem');
    expect(ruleFor('.thread-row', '').props.get('padding-right')).toBe(`var(${VAR})`);
    expect(RULES.filter(r => r.props.has(VAR)).map(r => r.selector))
      .toEqual(['.thread-drawer', '.thread-drawer']);
  });

  it('is what both dividers stop at, rather than a second copy', () => {
    const rowDivider = ruleFor('.thread-drawer .thread-row', '', '--row-hairline-right');
    expect(rowDivider.props.get('--row-hairline-right')).toBe(`var(${VAR})`);
    const shared = RULES.filter(r => r.props.has('--row-hairline-right')
      && selectorList(r.selector).includes('.thread-drawer .list-section-title-collapsible'));
    expect(shared).toHaveLength(1);
    expect(selectorList(shared[0].selector)).toContain('.thread-drawer .flip-disclosure-hairline');
    expect(shared[0].props.get('--row-hairline-right')).toBe(`var(${VAR})`);
  });

  it('grows on mobile, so the pin does not inherit the vacated corner', () => {
    expect(ruleFor('.thread-drawer', MOBILE, VAR).props.get(VAR)).toBe('var(--space-lg)');
  });

  it('is the only thing its mobile rule moves', () => {
    expect([...ruleFor('.thread-drawer', MOBILE, VAR).props.keys()]).toEqual([VAR]);
  });
});
