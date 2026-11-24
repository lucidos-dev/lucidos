import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { rulesTargeting } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const css: string = readFileSync(resolve(here, '../chat/input-messages.css'), 'utf8');

const runRules = rulesTargeting(css, 'icon-btn').filter((r) => r.selector.includes('turn-controls'));

const toneCss: string = readFileSync(resolve(here, '../global/host-components.css'), 'utf8');
const toneRule = (tone: string) => {
  const rules = rulesTargeting(toneCss, 'icon-btn')
    .filter((r) => r.selector === `.icon-btn[aria-pressed="true"][data-tone="${tone}"]`);
  expect(rules, `no ${tone} tone rule`).toHaveLength(1);
  return rules[0];
};

const pressedRules = runRules.filter((r) => r.selector.includes('aria-pressed="true"'));

describe('turn control pressed chip', () => {
  it('puts a pressed control on a neutral chip with a secondary glyph', () => {
    const quiet = toneRule('quiet');
    expect(quiet.props.get('background')).toBe('var(--bg-hover)');
    expect(quiet.props.get('color')).toBe('var(--text-secondary)');
  });

  it('draws no chip on an off control under the hover', () => {
    const hover = runRules.filter((r) => r.selector.includes(':hover'));
    expect(hover.length, 'no turn-control hover rule found').toBeGreaterThan(0);
    for (const rule of hover) {
      expect(rule.selector).toContain('[aria-pressed="false"]');
      expect(rule.props.get('background')).toBe('transparent');
    }
  });

  it('leaves the run no pressed paint of its own', () => {
    expect(pressedRules.map((r) => r.selector)).toEqual([]);
  });

  it('lights the folded collapse control in the accent, with no chip', () => {
    const glyph = toneRule('glyph');
    expect(glyph.props.get('color')).toBe('var(--accent)');
    expect(glyph.props.has('background'), 'the glyph tone draws a chip').toBe(false);
  });

  it('leaves the collapse control on the same muted default and hover as the pair', () => {
    const colours = runRules.filter((r) => r.props.has('color'));
    expect(colours.length, 'no base/hover colour rules found').toBeGreaterThan(0);
    for (const rule of colours) {
      expect(rule.selector, `"${rule.selector}" singles the collapse control out`)
        .not.toContain('turn-control-collapse');
    }
  });
});
