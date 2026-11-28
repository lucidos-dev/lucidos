
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { rulesTargeting, styleSheetPaths, type CssRule } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesRoot: string = resolve(here, '..');

function rulesFor(className: string): CssRule[] {
  const out: CssRule[] = [];
  for (const path of styleSheetPaths(stylesRoot)) {
    out.push(...rulesTargeting(readFileSync(path, 'utf-8'), className));
  }
  return out;
}

describe('a visually hidden label clips its own text', () => {
  it('is declared once, so there is one place this can regress', () => {
    const declaring = rulesFor('visually-hidden').filter(r => r.props.has('position'));
    expect(declaring.length, 'more than one rule builds the hidden box').toBe(1);
  });

  it('clips what it hides, so the text cannot become scrollable overflow', () => {
    const rule = rulesFor('visually-hidden').find(r => r.props.has('position'));
    expect(rule, 'no rule builds the hidden box at all').toBeDefined();
    expect(
      rule!.props.get('overflow'),
      'the hidden box does not clip: its text spills into the scroll container',
    ).toBe('hidden');
  });

  it('keeps the label on one line, so no column of characters forms', () => {
    const rule = rulesFor('visually-hidden').find(r => r.props.has('position'));
    expect(
      rule!.props.get('white-space'),
      'the label wraps, so a zero-width box stacks it one character per line',
    ).toBe('nowrap');
  });

  it('stays IN layout, which is why it cannot simply be display:none', () => {
    const rule = rulesFor('visually-hidden').find(r => r.props.has('position'));
    expect(rule!.props.get('display')).toBeUndefined();
    expect(rule!.props.get('visibility')).toBeUndefined();
    expect(rule!.props.get('position')).toBe('absolute');
  });
});
