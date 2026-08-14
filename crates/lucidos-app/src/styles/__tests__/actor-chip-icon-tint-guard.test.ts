/**
 * The coding-agent brand orange belongs to the coding agent's own logo, and to
 * nothing else in an actor chip.
 *
 * Both chip slots (`.initiator-icon` on the turn's initiator header,
 * `.response-executor-icon` on its response header) used to declare
 * `color: var(--initiator-coding-agent)` on `svg`, reaching every SVG child.
 * That was harmless purely by accident: at the time the only components in
 * those slots were the Lucidos mark, which paints itself from its own gradient,
 * and the Codex glyph, whose stroke is hardcoded, leaving the Claude logo as
 * the single icon the declaration actually painted.
 *
 * On 2026-08-13 the last four emoji in those slots became `currentColor`
 * components (the trigger bolt, the System power symbol, the You person, the
 * API-caller plug), so the accident stopped holding: an unscoped rule paints
 * "You" and "System" coding-agent orange. The rules are scoped to
 * `.claude-icon` now, and this pins that, because the failure is a colour on
 * one chip in one thread state and nothing else in the suite would catch it.
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
    it(`${file}: the brand orange is scoped to the Claude logo, never to every svg in .${slot}`, () => {
      const tinted = rulesTargeting(css, 'claude-icon').filter(
        r => r.selector.includes(slot) && r.props.get('color')?.includes('--initiator-coding-agent'),
      );
      expect(tinted.length, `expected a .${slot} .claude-icon colour rule`).toBe(1);

      // The actual regression shape: NO rule in the sheet may put the brand
      // colour anywhere that reaches the slot's other glyphs. Asserted over
      // every rule that carries the colour, so a re-widened selector, an
      // `@media` copy, or a brand-new rule all fail here. Reading the parsed
      // rules rather than the raw text is what makes that true of the whole
      // sheet instead of the first textual match.
      const carriers = cssRules(css).filter(r =>
        [...r.props.values()].some(v => v.includes('--initiator-coding-agent')),
      );
      expect(carriers.length, 'expected exactly one rule to carry the brand colour').toBe(1);
      expect(carriers[0].selector).toBe(`.${slot} .claude-icon`);
    });

    it(`${file}: .${slot} svg still sizes every chip glyph`, () => {
      const svgRule = css.match(new RegExp(`\\.${slot} svg\\s*\\{[^}]*\\}`))?.[0] ?? '';
      expect(svgRule).toContain('var(--icon-size-sm)');
    });
  }
});
