/**
 * The Claude mark's colour (`--claude-mark`) belongs to the Claude logo, and
 * to nothing else in an actor chip.
 *
 * Both chip slots (`.initiator-icon` on the turn's initiator header,
 * `.response-executor-icon` on its response header) also hold `currentColor`
 * glyphs: the trigger bolt, the System power symbol, the You person, the
 * API-caller plug. A rule on the slot's `svg` would paint "You" and "System"
 * the brand colour. So the rules are scoped to `.claude-icon`. This test pins
 * that, since nothing else in the suite sees a colour on one chip.
 *
 * A source scan rather than a browser test on purpose: the assertion is about
 * which SELECTOR carries the declaration, which is exactly what regresses when
 * someone folds the two rules back together, and a rendered check would need
 * one thread per actor to see it.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
import { cssRules, rulesTargeting } from './css-rule-helpers';

const SHEETS = [
  { file: 'initiator', css: readFileSync(new URL('../chat/input-messages.css', import.meta.url), 'utf8'), slot: 'initiator-icon' },
  { file: 'response', css: readFileSync(new URL('../chat/response.css', import.meta.url), 'utf8'), slot: 'response-executor-icon' },
];

describe('actor chip icon tint', () => {
  for (const { file, css, slot } of SHEETS) {
    it(`${file}: the Claude mark colour is scoped to the Claude logo, never to every svg in .${slot}`, () => {
      const tinted = rulesTargeting(css, 'claude-icon').filter(
        r => r.selector.includes(slot) && r.props.get('color')?.includes('--claude-mark'),
      );
      expect(tinted.length, `expected a .${slot} .claude-icon colour rule`).toBe(1);

      // The actual regression shape: NO rule in the sheet may put the brand
      // colour anywhere that reaches the slot's other glyphs. Asserted over
      // every rule that carries the colour, so a re-widened selector, an
      // `@media` copy, or a brand-new rule all fail here. Reading the parsed
      // rules rather than the raw text is what makes that true of the whole
      // sheet instead of the first textual match.
      const carriers = cssRules(css).filter(r =>
        [...r.props.values()].some(v => v.includes('--claude-mark')),
      );
      expect(carriers.length, 'expected exactly one rule to carry the Claude mark colour').toBe(1);
      expect(carriers[0].selector).toBe(`.${slot} .claude-icon`);
    });

    it(`${file}: .${slot} svg still sizes every chip glyph`, () => {
      const svgRule = css.match(new RegExp(`\\.${slot} svg\\s*\\{[^}]*\\}`))?.[0] ?? '';
      expect(svgRule).toContain('var(--icon-size-sm)');
    });
  }
});
