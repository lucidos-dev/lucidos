/**
 * Below one page of content, nothing in the transcript moves. The turns flow
 * from the top of the feed, and a turn already drawn stays where it was drawn
 * when the next one arrives.
 * Plan: docs/plans/2026-09-17-a-short-transcript-holds-still.md
 *
 * A source scan, because what it pins is which declarations are ABSENT. Where
 * the turns actually come to rest is measured instead, on every project, by
 * e2e/transcript-ends-where-its-content-ends.spec.ts.
 */

/* The rules banned here were live for one day, as `min-height: 100%` plus
 * `align-content: safe end` on `.thread-feed`. They closed the unused viewport
 * under a short thread by resting its turns on the bottom. Bottom-anchored, the
 * block of turns grows upward, so every new turn pushes everything above it up
 * the screen. Content the reader has already read travels, before there is a
 * page of it, on a transcript that cannot scroll at all. A call is where it is
 * loudest: every utterance is its own short turn.
 */

/* Four things are checked, and the last two are older than that reversal:
 *
 *   - nothing ALIGNS the feed or the scroller. Banning one half leaves the
 *     other in the tree, and the pair is one line from working again.
 *   - nothing FLOORS either one to a percentage. The desktop floor and its
 *     mobile chrome subtraction were the rest's other half, and a floor is what
 *     an alignment distributes.
 *   - `.thread-content` stays a BLOCK container. Safari ignores scrollTo on a
 *     flex-column scroll container, so the obvious flex rewrite of a rest
 *     breaks every transcript navigation on WebKit alone. styles/mobile.css
 *     carries the same warning at its padding override.
 *   - the rules read no follow or liveness state. A layout keyed on the follow
 *     flag turns every arming bug into a screenful of blank, which is report 2
 *     of docs/plans/2026-08-12-the-transcript-ends-where-its-content-ends.md.
 */
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

/** Every rule styling an element with `className`, across every sheet. */
function rulesFor(className: string): CssRule[] {
  const out: CssRule[] = [];
  for (const path of styleSheetPaths(stylesRoot)) {
    out.push(...rulesTargeting(readFileSync(path, 'utf-8'), className));
  }
  return out;
}

/** The two boxes a rest would have to live on: the turns' own box, and the
 *  scroll container holding it. */
const BOXES = ['thread-feed', 'thread-content'];

describe('a short transcript holds still', () => {
  // EVERY CHECK BELOW ASSERTS AN ABSENCE, so a scan that finds nothing at all
  // passes them without reading a byte of CSS. `.thread-feed` legitimately
  // carries no rule of its own now, and `.thread-content` carries several, so
  // the scroller is what proves the reader still works.
  it('finds the transcript rules it is scanning', () => {
    expect(
      rulesFor('thread-content').length,
      'the scan reads no rules, so every absence below is vacuous',
    ).toBeGreaterThan(0);
  });

  it('aligns neither the feed nor the scroller', () => {
    for (const box of BOXES) {
      for (const rule of rulesFor(box)) {
        for (const prop of ['align-content', 'justify-content', 'place-content']) {
          expect(
            rule.props.has(prop),
            `${rule.selector} sets ${prop}, which rests short content away from the top`,
          ).toBe(false);
        }
      }
    }
  });

  it('floors neither of them to a share of the pane', () => {
    for (const box of BOXES) {
      for (const rule of rulesFor(box)) {
        const floor = rule.props.get('min-height');
        if (floor === undefined) continue;
        expect(
          floor,
          `${rule.selector} floors the box to ${floor}, the room a rest rests against`,
        ).not.toMatch(/%/);
      }
    }
  });

  it('never makes the scroll container a flex or grid box', () => {
    for (const rule of rulesFor('thread-content')) {
      const display = rule.props.get('display');
      if (display === undefined) continue;
      expect(
        display,
        `${rule.selector} makes the transcript a ${display} box; Safari then ignores scrollTo on it`,
      ).not.toMatch(/\b(flex|grid|inline-flex|inline-grid)\b/);
    }
  });

  it('reads no follow, call or liveness state', () => {
    for (const box of BOXES) {
      for (const rule of rulesFor(box)) {
        expect(
          rule.selector,
          `${rule.selector} makes the layout depend on state the follow owns`,
        ).not.toMatch(/follow|live|riding|armed|calling/i);
      }
    }
  });
});
