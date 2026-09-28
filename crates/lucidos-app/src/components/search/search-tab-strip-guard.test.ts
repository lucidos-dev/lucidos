/**
 * The Search Everywhere categories wrap onto more lines instead of panning
 * sideways. A panning strip hid the later categories past the palette's edge.
 * On an iPhone the pan never landed, so they could not be reached at all.
 *
 * A source scan rather than a browser test: the emulators panned the strip
 * fine, so what can be pinned is which declarations are written.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

// `rulesTargeting` rather than the first-match string helpers, so a copy of the
// rule inside a media block cannot slip past.
import { rulesTargeting, type CssRule } from '../../styles/__tests__/css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(resolve(here, './SearchEverywhere.css'), 'utf-8');

const STRIP = 'search-everywhere-tabs';
const TAB = 'search-everywhere-tab';

/** Every rule styling the element, media copies and compound selectors alike. */
function rulesFor(className: string): CssRule[] {
  const rules = rulesTargeting(css, className);
  expect(rules.length, `no rule targets .${className}`).toBeGreaterThan(0);
  return rules;
}

/** The value the sheet lands on for `prop`: the last rule in it that sets one. */
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
