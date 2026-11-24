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
    const glyphs = ['exchange-status-x', 'exchange-status-warning', 'exchange-status-continued',
      'progress-dot-waiting', 'queued-message-edit', 'queued-message-remove'];
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
    const dot = cssRules(chatCss).find((r) => r.props.get('content') === "'\\00b7'");
    expect(dot?.props.get('position')).toBe('absolute');
    expect(dot?.props.get('left')).toBe('100%');
    expect(rulesTargeting(chatCss, 'response-timestamp').some((r) => r.selector.includes('::before')))
      .toBe(false);
  });

  it('both meta clusters clip past their right edge, and only a little', () => {
    for (const cluster of ['response-meta', 'initiator-meta']) {
      const clip = rulesTargeting(chatCss, cluster).find((r) => r.props.has('clip-path'));
      expect(clip?.props.get('clip-path'), `${cluster} does not clip`)
        .toBe('inset(-1rem -0.25rem -1rem -100vw)');
      expect(clip?.selector).toContain(`.${cluster}:not(:has(.exchange-status-glyph))`);
    }
  });
});
