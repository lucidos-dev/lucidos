

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

const BOXES = ['thread-feed', 'thread-content'];

describe('a short transcript holds still', () => {
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
