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
