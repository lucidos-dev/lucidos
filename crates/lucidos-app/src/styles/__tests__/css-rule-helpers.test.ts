import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';

import { cssRules, rulesTargeting, styleSheetPaths } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));

const selectorsMatching = (css: string, cls: string): string[] =>
  rulesTargeting(css, cls).map(r => r.selector);

describe('cssRules', () => {
  it('carries the at-rule preludes a rule is nested inside, outermost first', () => {
    const rules = cssRules(`
      .a { color: red; }
      @media (max-width: 768px) { @supports (display: grid) { .a { color: blue; } } }
    `);
    expect(rules.map(r => r.atRules)).toEqual([
      '',
      '@media (max-width: 768px) @supports (display: grid)',
    ]);
  });

  it('reads declarations as both a body string and a prop map, comments ignored', () => {
    const [rule] = cssRules('.a { /* why */ color: red; padding: 0 1rem; }');
    expect(rule.body).toBe('color: red; padding: 0 1rem');
    expect(rule.props.get('color')).toBe('red');
    expect(rule.props.get('padding')).toBe('0 1rem');
    expect(rule.props.has('why')).toBe(false);
  });
});

describe('rulesTargeting picks the rules that style the element itself', () => {
  it('takes the bare selector, a compound, and any ancestor combinator', () => {
    const css = `
      .t { a: 1; }
      .t.state { a: 2; }
      .parent .t { a: 3; }
      .parent > .t { a: 4; }
      .sibling + .t { a: 5; }
      .sibling~.t { a: 6; }
      .t:hover { a: 7; }
    `;
    expect(selectorsMatching(css, 't')).toHaveLength(7);
  });

  it('skips a rule aimed at a descendant or a pseudo-element', () => {
    const css = `
      .t .child { a: 1; }
      .t > .child { a: 2; }
      .t::-webkit-scrollbar { a: 3; }
      .t::before { a: 4; }
    `;
    expect(selectorsMatching(css, 't')).toEqual([]);
  });

  it('never matches a longer class that merely starts with the name', () => {
    expect(selectorsMatching('.tabs { a: 1; } .tab { a: 2; }', 'tab')).toEqual(['.tab']);
    expect(selectorsMatching('.tabs { a: 1; } .tab { a: 2; }', 'tabs')).toEqual(['.tabs']);
    expect(selectorsMatching('.tab-strip { a: 1; }', 'tab')).toEqual([]);
  });

  it('reads a selector list, including a comma inside :is()', () => {
    expect(selectorsMatching('.x, .t { a: 1; }', 't')).toEqual(['.x, .t']);
    expect(selectorsMatching('.t:is(.a, .b) { a: 1; }', 't')).toEqual(['.t:is(.a, .b)']);
    expect(selectorsMatching(':is(.a, .b) .child { a: 1; }', 'a')).toEqual([]);
  });

  it('reads a :has() argument as a condition, never as the subject', () => {
    expect(selectorsMatching('.owner:has(.t) { a: 1; }', 't')).toEqual([]);
    expect(selectorsMatching('.owner:has(.t) { a: 1; }', 'owner')).toEqual(['.owner:has(.t)']);
    expect(selectorsMatching('.t:has(.child) { a: 1; }', 't')).toEqual(['.t:has(.child)']);
    expect(selectorsMatching('.owner:has(:is(.a, .t)).state { a: 1; }', 't')).toEqual([]);
    expect(selectorsMatching('.owner:has(:is(.a, .t)).state { a: 1; }', 'state')).toEqual(['.owner:has(:is(.a, .t)).state']);
    expect(selectorsMatching('.owner:has(.a .t) .child { a: 1; }', 'child')).toEqual(['.owner:has(.a .t) .child']);
  });

  it('leaves combinators inside a functional pseudo-class alone', () => {
    expect(selectorsMatching('.t:is(.a > .b) { a: 1; }', 't')).toEqual(['.t:is(.a > .b)']);
    expect(selectorsMatching('[class~="x"].t { a: 1; }', 't')).toEqual(['[class~="x"].t']);
  });
});

describe('styleSheetPaths', () => {
  const paths = styleSheetPaths(resolve(here, '..'));

  it('finds every shipping sheet, including the nested ones', () => {
    const names = paths.map((p: string) => relative(resolve(here, '..'), p));
    expect(names).toContain('components.css');
    expect(names).toContain('global/base.css');
    expect(names.every((n: string) => n.endsWith('.css'))).toBe(true);
  });

  it('skips the test directory, so a fixture never reads as shipping CSS', () => {
    expect(paths.filter((p: string) => p.includes('__tests__'))).toEqual([]);
  });
});
