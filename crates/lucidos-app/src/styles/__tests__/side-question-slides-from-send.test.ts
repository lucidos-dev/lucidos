/**
 * Send opens into a split pill: the Side question half slides out of Send.
 *
 * The clip has to stay put at Send while the half moves. A clip animated on
 * the moving button reads as the button growing out of its own right end. So
 * the panel clips at Send's left edge and the half moves inside it. Send
 * squares off its left side behind a hairline, as a split button's caret does.
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

describe('Send opens into a split pill', () => {
  it('clips at Send\'s left edge, a fixed seam the half slides out of', () => {
    const panel = rule('.send-hold-menu');
    expect(panel.props.get('right')).toBe('var(--send-round-size)');
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

  it('draws the half as the pill\'s left end, as tall as Send', () => {
    const half = rule('.send-hold-menu > .action-btn');
    expect(half.props.get('height')).toBe('var(--send-round-size)');
    expect(half.props.get('border-radius')).toBe('var(--radius-round) 0 0 var(--radius-round)');
  });

  it('squares off Send\'s left side behind a hairline while the pill is open', () => {
    const send = rule('.prompt-actions-right > .send-cancel-round.split-open');
    expect(send.props.get('border-top-left-radius')).toBe('0');
    expect(send.props.get('border-bottom-left-radius')).toBe('0');
    expect(send.props.get('border-left')).toMatch(/^1px solid color-mix\(/);
  });

  it('rings the whole pill, whichever half holds focus', () => {
    // Send's ring loses its left side, the half's loses its right (the panel
    // clip), so the two meet at the seam as one outline.
    expect(rule('.prompt-actions-right > .send-cancel-round.split-open').props.get('clip-path'))
      .toMatch(/^inset\(-[\d.]+rem -[\d.]+rem -[\d.]+rem 0\)$/);
    const ringBoth = rule(
      '.prompt-actions-right:has(.send-hold-menu > .action-btn:focus-visible) > .send-cancel-morph, '
      + '.prompt-actions-right:has(> .send-cancel-round.split-open:focus-visible) .send-hold-menu > .action-btn',
    );
    expect(ringBoth.props.get('box-shadow')).toBe('var(--focus-ring)');
    // Forced colors strip the shadow and paint this outline, on both halves.
    expect(ringBoth.props.get('outline')).toBe('0.125rem solid transparent');
  });

  it('lightens the focused half, to say which one Enter presses', () => {
    const lit = rule(
      '.send-hold-menu > .action-btn:focus-visible, '
      + '.prompt-actions-right > .send-cancel-round.split-open:focus-visible',
    );
    expect(lit.props.get('background-color'))
      .toBe('color-mix(in srgb, var(--accent-action) 80%, var(--text-on-accent))');
  });

  it('draws Send over the half, so Send\'s focus ring shows at the seam', () => {
    const panelZ = Number(rule('.send-hold-menu').props.get('z-index'));
    const sendZ = Number(rule('.prompt-actions-right > .send-cancel-morph').props.get('z-index'));
    expect(sendZ).toBeGreaterThan(panelZ);
  });
});
