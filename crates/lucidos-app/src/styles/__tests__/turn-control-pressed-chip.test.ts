/**
 * A pressed turn toggle sits on a chip; the collapse control never does.
 *
 * All three turn controls report `aria-pressed`, and one rule keys the chip
 * off it. For the transcript-wide pair the chip means "on, and you are seeing
 * MORE". On the collapse control, in either header's `.turn-controls` run, it
 * would mean FOLDED: the same cue inverted, beside the two it contradicts.
 *
 * So the collapse control carries its state in its GLYPH: the plus (pinned in
 * components/chat/__tests__/turn-controls.test.tsx), drawn in the accent. A
 * fold draws nothing under the header, so that lit plus is the only mark of it.
 * `aria-pressed` stays on it for a screen reader, which is why the chip's
 * exclusion is written into the selector: drop the `:not()` and the attribute
 * alone draws the chip again.
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

/** Rules that style a `.turn-controls` icon button by its pressed state. */
const pressedRules = runRules.filter((r) => r.selector.includes('aria-pressed="true"'));

/** Rules that style the folded collapse control, rather than exclude it. */
const foldedRules = rulesTargeting(css, 'turn-control-collapse')
  .filter((r) => r.selector.includes('.turn-control-collapse[aria-pressed="true"]'));

describe('turn control pressed chip', () => {
  it('puts a pressed control on a neutral chip with a secondary glyph', () => {
    const chip = pressedRules.filter((r) => r.props.has('background'));
    expect(chip.length, 'no aria-pressed chip rule found').toBeGreaterThan(0);
    for (const rule of chip) {
      // The quiet hover grey, never the app bar's accent chip: these repeat on
      // every turn, and a column of accent chips reads as a column of alerts.
      expect(rule.props.get('background')).toBe('var(--bg-hover)');
      expect(rule.props.get('color')).toBe('var(--text-secondary)');
    }
  });

  it('draws no chip on hover, so the chip only ever means pressed', () => {
    // The shared `.icon-btn` hover chip is the pressed chip's colour, so an
    // off control under the cursor would read as on.
    const hover = runRules.filter((r) => r.selector.includes(':hover') && !r.selector.includes('aria-pressed'));
    expect(hover.length, 'no turn-control hover rule found').toBeGreaterThan(0);
    for (const rule of hover) expect(rule.props.get('background')).toBe('transparent');
  });

  it('excludes the collapse control from every pressed chip rule', () => {
    const chip = pressedRules.filter((r) => r.props.has('background'));
    expect(chip.length).toBeGreaterThan(0);
    for (const rule of chip) {
      expect(
        rule.selector,
        `"${rule.selector}" gives the collapse control a chip; it states its state by its glyph`,
      ).toContain(':not(.turn-control-collapse)');
    }
  });

  it('lights the folded collapse control in the accent, with no chip', () => {
    expect(foldedRules.length, 'no folded collapse-control rule found').toBeGreaterThan(0);
    for (const rule of foldedRules) {
      expect(rule.props.get('color')).toBe('var(--accent)');
      expect(rule.props.has('background'), `"${rule.selector}" draws a chip`).toBe(false);
    }
  });

  it('leaves the collapse control on the same muted default and hover as the pair', () => {
    // The exclusion is from the ON look only. Everything else about the
    // control is shared. So it still reads as one of three icons in a row,
    // not as a disabled or differently-skinned button.
    const colours = runRules.filter((r) => !r.selector.includes('aria-pressed') && r.props.has('color'));
    expect(colours.length, 'no base/hover colour rules found').toBeGreaterThan(0);
    for (const rule of colours) {
      expect(rule.selector, `"${rule.selector}" singles the collapse control out`)
        .not.toContain('turn-control-collapse');
    }
  });
});
