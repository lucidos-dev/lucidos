/**
 * `rulesTargeting` is the half of the shared CSS reader with judgment in it: it
 * decides which rules style an ELEMENT as opposed to one of its descendants or
 * pseudo-elements. Seven suites now lean on that decision to assert "nothing in
 * this sheet re-enables X", and a matcher that is too narrow makes such a scan
 * pass while the override it was written to catch sits right there in the file.
 *
 * So the semantics are pinned here rather than left implicit in each caller.
 * `block` / `decl` are covered incidentally by every suite that uses them; the
 * subject derivation is not, because a caller cannot see it go wrong.
 */
import { describe, it, expect } from 'vitest';

import { cssRules, rulesTargeting } from './css-rule-helpers';

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
    // The regression this exists for: `.tabs` and `.tab` are both real classes
    // in SearchEverywhere.css, and one must not answer for the other.
    expect(selectorsMatching('.tabs { a: 1; } .tab { a: 2; }', 'tab')).toEqual(['.tab']);
    expect(selectorsMatching('.tabs { a: 1; } .tab { a: 2; }', 'tabs')).toEqual(['.tabs']);
    expect(selectorsMatching('.tab-strip { a: 1; }', 'tab')).toEqual([]);
  });

  it('reads a selector list, including a comma inside :is()', () => {
    expect(selectorsMatching('.x, .t { a: 1; }', 't')).toEqual(['.x, .t']);
    // A plain comma split would cut this into `.t:is(.a` and `.b)`, and the
    // second fragment is not a subject carrying `.t`.
    expect(selectorsMatching('.t:is(.a, .b) { a: 1; }', 't')).toEqual(['.t:is(.a, .b)']);
    // Here the subject is `.child`, so the rule styles a descendant.
    expect(selectorsMatching(':is(.a, .b) .child { a: 1; }', 'a')).toEqual([]);
  });

  it('reads a :has() argument as a condition, never as the subject', () => {
    // `:has()` is the one functional pseudo-class whose argument describes
    // something OTHER than the element being styled, so a class named there must
    // not answer for it. The regression: the transcript's tail room was reserved
    // on `.chat-exchange:last-child:has(.response-header)`, and its `min-height`
    // was read as the response header's own, failing an unrelated scan that
    // pins what floors that header.
    expect(selectorsMatching('.owner:has(.t) { a: 1; }', 't')).toEqual([]);
    expect(selectorsMatching('.owner:has(.t) { a: 1; }', 'owner')).toEqual(['.owner:has(.t)']);
    // Still the subject when it is genuinely the subject as well.
    expect(selectorsMatching('.t:has(.child) { a: 1; }', 't')).toEqual(['.t:has(.child)']);
    // A nested paren closes with its own owner, so the rest of the compound survives.
    expect(selectorsMatching('.owner:has(:is(.a, .t)).state { a: 1; }', 't')).toEqual([]);
    expect(selectorsMatching('.owner:has(:is(.a, .t)).state { a: 1; }', 'state')).toEqual(['.owner:has(:is(.a, .t)).state']);
    // And a descendant combinator INSIDE the argument is not one of the subject's.
    expect(selectorsMatching('.owner:has(.a .t) .child { a: 1; }', 'child')).toEqual(['.owner:has(.a .t) .child']);
  });

  it('leaves combinators inside a functional pseudo-class alone', () => {
    // The `>` belongs to the :is() argument, so the subject is still `.t`.
    expect(selectorsMatching('.t:is(.a > .b) { a: 1; }', 't')).toEqual(['.t:is(.a > .b)']);
    // The `~` is an attribute operator, not a sibling combinator.
    expect(selectorsMatching('[class~="x"].t { a: 1; }', 't')).toEqual(['[class~="x"].t']);
  });
});
