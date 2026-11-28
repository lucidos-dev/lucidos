import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules, rulesTargeting, selectorList, type CssRule } from '../../styles/__tests__/css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(resolve(here, './SearchEverywhere.css'), 'utf-8');

const STRIP = 'search-everywhere-tabs';
const TAB = 'search-everywhere-tab';

function rulesFor(className: string): CssRule[] {
  const rules = rulesTargeting(css, className);
  expect(rules.length, `no rule targets .${className}`).toBeGreaterThan(0);
  return rules;
}

function effective(className: string, prop: string): string | undefined {
  let value: string | undefined;
  for (const rule of rulesFor(className)) {
    const v = rule.props.get(prop);
    if (v !== undefined) value = v;
  }
  return value;
}

describe('the Search Everywhere categories are all on screen', () => {
  it('wraps the chips onto more lines', () => {
    expect(effective(STRIP, 'flex-wrap')).toBe('wrap');
  });

  it('never makes the strip a scroller, however the rule is reached', () => {
    for (const rule of rulesFor(STRIP)) {
      for (const prop of ['overflow', 'overflow-x', 'overflow-y']) {
        expect(rule.props.get(prop), `${rule.selector} { ${prop} } under ${rule.atRules || 'top level'}`)
          .toBeUndefined();
      }
    }
  });

  it('never gives the strip\'s space to the results list', () => {
    expect(effective(STRIP, 'flex-shrink')).toBe('0');
  });

  it('keeps each label on one line', () => {
    expect(effective(TAB, 'white-space')).toBe('nowrap');
  });
});

describe('the first chip holds its width as the query starts', () => {
  const WIDEST = 'search-everywhere-widest';
  const prop = (selector: string, name: string): string | undefined =>
    cssRules(css)
      .filter(r => selectorList(r.selector).includes(selector))
      .map(r => r.props.get(name))
      .find(v => v !== undefined);

  it('lays the widest content invisibly in the box', () => {
    expect(prop(`.${WIDEST}::before`, 'content')).toBe('attr(data-widest)');
    expect(prop(`.${WIDEST}::before`, 'visibility')).toBe('hidden');
  });

  it('stacks the real content on the widest one', () => {
    expect(effective(WIDEST, 'display')).toBe('inline-grid');
    expect(prop(`.${WIDEST}::before`, 'grid-area')).toBe('1 / 1');
    expect(prop(`.${WIDEST} > *`, 'grid-area')).toBe('1 / 1');
  });
});
