/**
 * The turn's collapse control never brightens; the two beside it do.
 *
 * All three turn controls report `aria-pressed`, and one rule keys
 * the brightened "on" look off that attribute. Bright means "on, and you are
 * seeing MORE" for the transcript-wide pair, whose off state looks like nothing
 * at all: a turn drawing no steps is indistinguishable from a turn whose steps
 * are hidden unless the control says which. On the collapse control the same
 * cue would mean the turn is FOLDED, which is that meaning inverted, sitting
 * 0.125rem from the two it contradicts, and reporting a state nothing is
 * hiding: the turn underneath has become a `⋯` stub. Reported as "weird that it
 * toggles between gray and white".
 *
 * The rule reaches the INITIATOR header's lone collapse control too, since both
 * headers put their controls in a `.turn-controls` run. It has to keep missing
 * it there for the same reason.
 *
 * So it carries its state in its GLYPH instead (the arrowheads turn around,
 * pinned in components/chat/__tests__/turn-controls.test.tsx), and the
 * brightness rule excludes it. `aria-pressed` stays on the element for a screen
 * reader, which is exactly why the exclusion has to be written into the
 * selector: drop the `:not()` and the attribute alone lights the icon again.
 *
 * A source scan because nothing else can see it. `tsc` does not read CSS,
 * `vite build` only parses it, and the regression is a colour on one of three
 * icons. Parsed with postcss rather than matched textually, so a second rule
 * re-lighting the control from anywhere in the sheet is caught too.
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

/** Rules that colour a `.turn-controls` icon button by its pressed state. */
const pressedRules = rulesTargeting(css, 'icon-btn').filter(
  (r) => r.selector.includes('turn-controls')
    && r.selector.includes('aria-pressed="true"')
    && r.props.has('color'),
);

describe('turn control brightness', () => {
  it('brightens a pressed control, which is the pair\'s whole on-state', () => {
    expect(pressedRules.length, 'no aria-pressed colour rule found').toBeGreaterThan(0);
    for (const rule of pressedRules) {
      // Muted to bright, deliberately not the accent-on-a-chip the app bar's
      // active icons wear: these repeat on every turn, and a column of filled
      // accent chips down a long thread reads as a column of alerts.
      expect(rule.props.get('color')).toBe('var(--text-primary)');
    }
  });

  it('excludes the collapse control from every one of those rules', () => {
    for (const rule of pressedRules) {
      expect(
        rule.selector,
        `"${rule.selector}" lights the collapse control; it states its state by turning its arrows around`,
      ).toContain(':not(.turn-control-collapse)');
    }
  });

  it('leaves the collapse control on the same muted default and hover as the pair', () => {
    // The exclusion is from the ON colour only. Everything else about the
    // control is shared, so it still reads as one of three icons in a row
    // rather than as a disabled or differently-skinned button.
    const shared = rulesTargeting(css, 'icon-btn').filter(
      (r) => r.selector.includes('turn-controls') && !r.selector.includes('aria-pressed'),
    );
    const colours = shared.filter((r) => r.props.has('color'));
    expect(colours.length, 'no base/hover colour rules found').toBeGreaterThan(0);
    for (const rule of colours) {
      expect(rule.selector, `"${rule.selector}" singles the collapse control out`)
        .not.toContain('turn-control-collapse');
    }
  });
});
