/**
 * The thread filter panel's section headings stand at one height, ticked or not.
 *
 * "By thread types" grows a checkmark the moment a thread type is ticked, and
 * that mark is a --icon-size-md glyph joining a --font-size-md line. Under a
 * `normal` line height the row was as tall as its own text, so the glyph made it
 * 1px taller: the heading's words dropped half a pixel and every thread type
 * under it dropped a whole one, on the very toggle the user is looking at. The
 * heading now reserves a glyph's height unconditionally.
 *
 * A stylesheet property, so it is pinned here rather than in a browser: jsdom
 * lays nothing out, and the panel's own suite (ThreadFilterPanel.test.tsx) can
 * only see that the mark renders, never that it costs the row nothing.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, decl, rulesTargeting } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const drawerCss: string = readFileSync(resolve(here, '../drawer.css'), 'utf-8');

describe('the filter heading reserves a glyph row, checkmark or not', () => {
  it('pins the line box to the mark itself, not to a ratio of the text', () => {
    // The two tokens are what have to agree. A ratio (`line-height: 1.25`)
    // covers the mark at today's values and silently stops the day either token
    // moves, which reads as a passing scan and a heading that jumps again.
    expect(decl(block(drawerCss, '.thread-filter-title {'), 'line-height'))
      .toBe('var(--icon-size-md)');
  });

  it('the mark is sized off that same token', () => {
    const glyph = block(drawerCss, '.thread-filter-title-check svg {');
    for (const axis of ['width', 'height']) {
      expect(decl(glyph, axis), `a ${axis} of its own would outgrow the row it sits in`)
        .toBe('var(--icon-size-md)');
    }
  });

  it('no state re-introduces a height the checkmark can change', () => {
    // `-active` and `-dimmed` ride alongside the base class, so a height on
    // either would be a per-state row height: exactly the bug, in a new place.
    // `rulesTargeting` reads the element's own rules, so the `-check` child and
    // those sibling classes are correctly out of scope here.
    const heading = rulesTargeting(drawerCss, 'thread-filter-title');
    const sized = heading
      .filter(r => ['height', 'min-height', 'max-height'].some(p => r.props.has(p)));
    expect(sized.map(r => `${r.atRules} ${r.selector}`)).toEqual([]);

    const pinned = heading.filter(r => r.props.has('line-height'));
    expect(pinned.map(r => r.selector), 'one line-height, or the headings disagree in some state')
      .toEqual(['.thread-filter-title']);
    expect(pinned[0].atRules, 'a viewport-conditional copy is a second height in hiding').toBe('');
  });
});
