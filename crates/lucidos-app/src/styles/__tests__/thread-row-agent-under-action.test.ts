/**
 * The drawer row's agent mark stands under the title line's last action
 * button. Both end at the row's right edge, so their centres meet only when
 * their boxes are equally wide. Each layout sets one width var on the row, and
 * both boxes read it.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

import { cssRules } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const RULES = cssRules(readFileSync(resolve(here, '../drawer.css'), 'utf8'));
const VAR = '--thread-row-action-width';
const LAYOUTS = ['@media (--desktop-layout)', '@media (--phone-layout)'];

function widthOf(sel: string, atRules: string) {
  const found = RULES.filter(r => r.selector === sel && r.atRules === atRules && r.props.has('width'));
  expect(found, `no width rule "${sel}" under "${atRules || 'top level'}"`).toHaveLength(1);
  return found[0].props.get('width');
}

describe('the drawer row agent mark', () => {
  it('is as wide as the last action button, with its glyph centred', () => {
    expect(widthOf('.thread-row-agent', '')).toBe(`var(${VAR})`);
    const mark = RULES.find(r => r.selector === '.thread-row-agent' && r.atRules === '')!;
    expect(mark.props.get('justify-content')).toBe('center');
    for (const layout of LAYOUTS) {
      expect(widthOf('.thread-row-actions .icon-btn.header-icon', layout)).toBe(`var(${VAR})`);
    }
  });

  it('takes its width from the row in every layout', () => {
    for (const layout of LAYOUTS) {
      const set = RULES.filter(r => r.selector === '.thread-row' && r.atRules === layout && r.props.has(VAR));
      expect(set, `no ${VAR} on .thread-row under ${layout}`).toHaveLength(1);
    }
  });
});
