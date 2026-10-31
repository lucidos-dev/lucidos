/**
 * A pressed turn toggle sits on a chip; the collapse control never does.
 *
 * The transcript-wide pair wears the `quiet` tone (a neutral chip), and the
 * collapse control wears the `glyph` tone (accent, no chip). For the pair the
 * chip means "on, and you are seeing MORE". On the collapse control, in either
 * header's `.turn-controls` run, it would mean FOLDED: the same cue inverted,
 * beside the two it contradicts.
 *
 * So the collapse control carries its state in its GLYPH: the plus (pinned in
 * components/chat/__tests__/turn-controls.test.tsx), drawn in the accent. A
 * fold draws nothing under the header, so that lit plus is the only mark of it.
 *
 * A source scan because `tsc` does not read CSS and `vite build` only parses
 * it. Parsed with postcss, so a second rule anywhere in the sheet is caught.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { rulesTargeting } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
// The run's rules live with the actor/executor chip they are measured against,
// which both headers share, rather than with either header.
const css: string = readFileSync(resolve(here, '../chat/input-messages.css'), 'utf8');

const runRules = rulesTargeting(css, 'icon-btn').filter((r) => r.selector.includes('turn-controls'));

const toneCss: string = readFileSync(resolve(here, '../global/host-components.css'), 'utf8');
const toneRule = (tone: string) => {
  const rules = rulesTargeting(toneCss, 'icon-btn')
    .filter((r) => r.selector === `.icon-btn[aria-pressed="true"][data-tone="${tone}"]`);
  expect(rules, `no ${tone} tone rule`).toHaveLength(1);
  return rules[0];
};

/** No rule of the run's own styles a pressed control: its look is a tone. */
const pressedRules = runRules.filter((r) => r.selector.includes('aria-pressed="true"'));

describe('turn control pressed chip', () => {
  it('puts a pressed control on a neutral chip with a secondary glyph', () => {
    // The quiet hover grey, never the app bar's accent chip: these repeat on
    // every turn, and a column of accent chips reads as a column of alerts.
    const quiet = toneRule('quiet');
    expect(quiet.props.get('background')).toBe('var(--bg-hover)');
    expect(quiet.props.get('color')).toBe('var(--text-secondary)');
  });

  it('draws no chip on an off control under the hover', () => {
    // The shared `.icon-btn` hover chip is the pressed chip's colour, so an
    // off control under the cursor would read as on. The rule names the off
    // state, which is what lets a pressed control keep its chip.
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
    // The ON look is the tone's. Everything else about the control is shared.
    // So it still reads as one of three icons in a row, not as a disabled or
    // differently-skinned button.
    const colours = runRules.filter((r) => r.props.has('color'));
    expect(colours.length, 'no base/hover colour rules found').toBeGreaterThan(0);
    for (const rule of colours) {
      expect(rule.selector, `"${rule.selector}" singles the collapse control out`)
        .not.toContain('turn-control-collapse');
    }
  });
});
