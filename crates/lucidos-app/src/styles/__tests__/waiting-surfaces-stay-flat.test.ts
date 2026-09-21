/**
 * Three reports about the same wait, all of them geometry.
 *
 * The panel drew a filled row inside its own filled box, a card in a card. The
 * event type under the reason sat centred, because the UA centres a button's
 * text and a filtered entry is a button. And the folded ⋯ row carries the
 * model's own sentence, which made a `max-content` menu wider than the phone:
 * `computeAnchorPosition` can only clamp a panel that fits, so it pinned to the
 * left margin and ran off the right edge.
 *
 * Scanned rather than measured. Each failure is a property of the rule, and
 * reproducing one needs a live wait, a long event type and a narrow viewport.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { rulesTargeting } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const read = (rel: string): string => readFileSync(resolve(here, rel), 'utf-8');
const waitingCss = read('../chat/waiting-indicator.css');
const hostCss = read('../global/host-components.css');

/** Every pseudo-class that means a pointer or a finger is on the row. A tint
 *  under one of those is feedback; the same fill at rest is a card. */
const INTERACTING = /:hover|:active|:focus/;

function restingFills(css: string, className: string): string[] {
  return rulesTargeting(css, className)
    .filter((rule) => !INTERACTING.test(rule.selector) && !rule.atRules.includes('hover'))
    .map((rule) => rule.props.get('background') ?? rule.props.get('background-color'))
    .filter((value): value is string => value !== undefined && value !== 'none');
}

describe('the waiting panel draws no card inside its own card', () => {
  it('leaves a subscription row unfilled', () => {
    expect(restingFills(waitingCss, 'event-wait-item')).toEqual([]);
  });

  /** Flat is not the same as dead. A row that answers nothing on press reads as
   *  text, and on touch there is no hover to fall back on. */
  it('leaves a sub-thread row unfilled until a pointer or a finger is on it', () => {
    expect(restingFills(waitingCss, 'waiting-panel-child-link')).toEqual([]);
    const lit = rulesTargeting(waitingCss, 'waiting-panel-child-link')
      .filter((rule) => INTERACTING.test(rule.selector))
      .map((rule) => rule.selector);
    expect(lit, 'the row lost its press feedback with its fill')
      .toEqual(expect.arrayContaining([
        expect.stringContaining(':hover'),
        expect.stringContaining(':active'),
      ]));
  });
});

describe('a wrapped event type reads on the same axis as its reason', () => {
  it('never lets the UA centre the filtered entry', () => {
    const centred = rulesTargeting(waitingCss, 'event-wait-subscription-filter')
      .filter((rule) => {
        const align = rule.props.get('text-align');
        return align !== undefined && align !== 'inherit' && align !== 'left' && align !== 'start';
      });
    expect(centred.map((r) => r.body)).toEqual([]);
    const aligned = rulesTargeting(waitingCss, 'event-wait-subscription-filter')
      .some((rule) => rule.props.has('text-align'));
    expect(aligned, 'the button takes the UA centre without a text-align').toBe(true);
  });
});

describe('a folded action row stays inside the menu', () => {
  it('caps the menu against the viewport', () => {
    const cap = rulesTargeting(hostCss, 'thread-overflow-menu')
      .map((rule) => rule.props.get('max-width'))
      .find((value) => value !== undefined);
    expect(cap, '.thread-overflow-menu sizes to max-content with no ceiling').toBeDefined();
    expect(cap).toContain('100vw');
  });

  it('lets a row that reaches the cap wrap', () => {
    const nowrap = rulesTargeting(hostCss, 'thread-overflow-item')
      .filter((rule) => rule.props.get('white-space') === 'nowrap');
    expect(nowrap.map((r) => `${r.selector} { ${r.body} }`)).toEqual([]);
  });

  /** Two lines, then an ellipsis. Wrapping alone let one status readout fill
   *  five lines of a three-row menu. */
  it('stops the label after two lines', () => {
    const label = rulesTargeting(hostCss, 'thread-overflow-label')[0];
    expect(label, 'no rule clamps a folded action label').toBeDefined();
    expect(label.props.get('-webkit-line-clamp')).toBe('2');
    expect(label.props.get('overflow')).toBe('hidden');
    // Without it the box takes its longest word as a floor, and the row pushes
    // the menu wider than the cap instead of clamping.
    expect(label.props.get('min-width')).toBe('0');
  });
});
