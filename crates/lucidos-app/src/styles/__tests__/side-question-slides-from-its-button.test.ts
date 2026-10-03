/**
 * The row's end button opens into a split pill: the Side question half slides
 * out of it. That button is Send, Stop, or Submit while a question card waits.
 *
 * The clip has to stay put at the button while the half moves. A clip animated
 * on the moving half reads as the half growing out of its own right end. So
 * the panel clips at the button's left edge and the half moves inside it. The
 * button squares off its left side behind a hairline, as a split button's
 * caret does.
 *
 * The half measures against `.split-pill`, which wraps the button alone. A
 * fixed offset fitted only Send, and covered the wider Submit.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, cssRules, decl } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(resolve(here, '../chat/input-messages.css'), 'utf-8');
const rules = cssRules(css);
const rule = (selector: string) => {
  const found = rules.find(r => r.selector === selector);
  expect(found, `no rule for ${selector}`).toBeDefined();
  return found!;
};

describe('the row\'s end button opens into a split pill', () => {
  it('measures the half against the button it slides out of, whatever its width', () => {
    const pill = rule('.split-pill');
    expect(pill.props.get('position')).toBe('relative');
    expect(rule('.send-hold-menu').props.get('right')).toBe('100%');
  });

  it('takes no gap in the row while it holds nothing', () => {
    // Banner buttons can own the end of the row. The wrapper stays mounted so
    // the pill still runs its close, and the fold counts no gap for it.
    expect(rule('.split-pill:empty').props.get('display')).toBe('none');
  });

  it('opens no stacking context, so the half draws over the row\'s neighbours', () => {
    // A neighbour such as Archive sits on its own layer at z-index 1. A
    // stacking context here would trap the half beneath it.
    expect(rule('.split-pill').props.has('z-index')).toBe(false);
  });

  it('clips at the button\'s left edge, a fixed seam the half slides out of', () => {
    const panel = rule('.send-hold-menu');
    // Only the seam clips. The rest leaves room for the focus ring.
    expect(panel.props.get('clip-path')).toMatch(/^inset\(-[\d.]+rem 0 -[\d.]+rem -[\d.]+rem\)$/);
    expect(panel.props.has('animation'), 'the clip must not move').toBe(false);
  });

  it('moves the half from wholly behind the seam', () => {
    const half = rule('.send-hold-menu > .action-btn');
    expect(half.props.get('animation')).toMatch(/^send-hold-slide-out /);
    const from = block(block(css, '@keyframes send-hold-slide-out'), 'from');
    expect(decl(from, 'transform')).toBe('translateX(100%)');
    expect(decl(from, 'clip-path'), 'a clip on the mover grows it in place').toBeNull();
  });

  it('slides the half back behind the seam when the pill shuts', () => {
    const leaving = rule('.send-hold-menu-leaving > .action-btn');
    expect(leaving.props.get('animation')).toMatch(/^send-hold-slide-in var\(--duration-normal\) ease-in forwards$/);
    const to = block(block(css, '@keyframes send-hold-slide-in'), 'to');
    expect(decl(to, 'transform')).toBe('translateX(100%)');
    // A drawing only: the pill has shut, so nothing here may take a press.
    expect(rule('.send-hold-menu-leaving').props.get('pointer-events')).toBe('none');
  });

  it('draws the half as tall as the button, with the button\'s own corners', () => {
    const half = rule('.send-hold-menu > .action-btn');
    expect(half.props.get('height')).toBe('100%');
    expect(half.props.get('border-radius')).toBe('var(--split-pill-radius) 0 0 var(--split-pill-radius)');
    // An `.action-btn` such as Submit by default, a round end beside Send.
    expect(rule('.split-pill').props.get('--split-pill-radius')).toBe('calc(var(--radius-control) * 0.5)');
    expect(rule('.split-pill:has(> .send-cancel-round)').props.get('--split-pill-radius')).toBe('var(--radius-round)');
  });

  it('squares off the button\'s left side behind a hairline while the pill is open', () => {
    const button = rule('.split-pill > .split-open');
    expect(button.props.get('border-top-left-radius')).toBe('0');
    expect(button.props.get('border-bottom-left-radius')).toBe('0');
    expect(rule('.send-hold-menu > .action-btn').props.get('border-right')).toMatch(/^1px solid color-mix\(/);
  });

  it('opens without widening the button, so the row\'s fold has nothing to re-measure', () => {
    // Submit sizes to its label, so a border on it would add its width.
    const button = rule('.split-pill > .split-open');
    for (const prop of ['border', 'border-left', 'border-left-width', 'padding', 'padding-left', 'width', 'margin', 'margin-left']) {
      expect(button.props.has(prop), `.split-open must not set ${prop}`).toBe(false);
    }
  });

  it('rings the whole pill, whichever half holds focus', () => {
    // The button's ring loses its left side, the half's loses its right (the
    // panel clip), so the two meet at the seam as one outline.
    expect(rule('.split-pill > .split-open').props.get('clip-path'))
      .toMatch(/^inset\(-[\d.]+rem -[\d.]+rem -[\d.]+rem 0\)$/);
    const ringBoth = rule(
      '.split-pill:has(> .send-hold-menu > .action-btn:focus-visible) > .action-btn, '
      + '.split-pill:has(> .split-open:focus-visible) > .send-hold-menu > .action-btn',
    );
    expect(ringBoth.props.get('box-shadow')).toBe('var(--focus-ring)');
    // Forced colors strip the shadow and paint this outline, on both halves.
    expect(ringBoth.props.get('outline')).toBe('0.125rem solid transparent');
  });

  it('lightens the focused half from its own colour, to say which one Enter presses', () => {
    const lit = rule('.send-hold-menu > .action-btn:focus-visible, .split-pill > .split-open:focus-visible');
    expect(lit.props.get('background-color'))
      .toBe('color-mix(in srgb, var(--accent-action) 80%, var(--text-on-accent))');
    expect(rule('.split-pill > .split-open.action-btn-confirm:focus-visible').props.get('background-color'))
      .toBe('color-mix(in srgb, var(--confirm-bg) 80%, var(--text-on-accent))');
  });

  it('draws the button over the half, so the button\'s focus ring shows at the seam', () => {
    const panelZ = Number(rule('.send-hold-menu').props.get('z-index'));
    const buttonZ = Number(rule('.split-pill > .action-btn').props.get('z-index'));
    expect(buttonZ).toBeGreaterThan(panelZ);
  });
});
