/**
 * The dot between a turn's status and its timestamp shows only where it parts
 * two things on one line.
 *
 * It drops in two cases. A status ending in a glyph (✕, a warning triangle,
 * the waiting dot, ↳, the queued bin) is parted by that glyph already. A pair
 * stacked on two lines has nothing on the line to part.
 *
 * CSS decides both, with no measuring script. The dot hangs off the status,
 * and the meta cluster clips past its right edge. A stacked status sits flush
 * on that edge, so its dot falls outside. jsdom runs no layout, so this pins
 * the wiring as a source scan.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { cssRules, rulesTargeting } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const chatCss: string = readFileSync(resolve(here, '../chat/input-messages.css'), 'utf8');
const chatExchange: string = readFileSync(
  resolve(here, '../../components/chat/ChatExchange.tsx'),
  'utf8',
);

describe('a status glyph replaces the dot', () => {
  it('every glyph a status label can end in carries the marker class', () => {
    // The CSS keys on the marker, so a new glyph without it draws both.
    const glyphs = ['exchange-status-x', 'exchange-status-warning', 'exchange-status-continued',
      'progress-dot-waiting', 'queued-message-remove'];
    for (const glyph of glyphs) {
      const uses = chatExchange.match(new RegExp(`class(Name)?="[^"]*\\b${glyph}\\b[^"]*"`, 'g')) ?? [];
      expect(uses.length, `${glyph} is gone from ChatExchange`).toBeGreaterThan(0);
      for (const use of uses) expect(use, `${glyph} lacks the marker`).toContain('exchange-status-glyph');
    }
  });

  it('only a glyph-free status draws the dot', () => {
    const dots = cssRules(chatCss).filter((r) => r.props.get('content') === "'\\00b7'");
    expect(dots.length, 'the dot rule is gone').toBe(1);
    expect(dots[0].selector).toContain(':not(:has(.exchange-status-glyph))');
    expect(dots[0].selector).toContain('::after');
  });
});

describe('a stacked pair draws no dot', () => {
  it('the dot hangs off the status, out of flow', () => {
    // In flow on the timestamp, it led line two once the pair stacked.
    const dot = cssRules(chatCss).find((r) => r.props.get('content') === "'\\00b7'");
    expect(dot?.props.get('position')).toBe('absolute');
    expect(dot?.props.get('left')).toBe('100%');
    expect(rulesTargeting(chatCss, 'response-timestamp').some((r) => r.selector.includes('::before')))
      .toBe(false);
  });

  it('both meta clusters clip past their right edge, and only a little', () => {
    // The right inset is the focus-ring slack. The dot centres 0.625rem past
    // the edge, so a wider slack would let a stacked status show its dot.
    for (const cluster of ['response-meta', 'initiator-meta']) {
      const clip = rulesTargeting(chatCss, cluster).find((r) => r.props.has('clip-path'));
      expect(clip?.props.get('clip-path'), `${cluster} does not clip`)
        .toBe('inset(-1rem -0.25rem -1rem -1rem)');
      // A glyph draws no dot, and the queued bin's chip needs more slack.
      expect(clip?.selector).toContain(`.${cluster}:not(:has(.exchange-status-glyph))`);
    }
  });
});
