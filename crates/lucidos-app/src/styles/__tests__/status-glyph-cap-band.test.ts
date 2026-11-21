/**
 * The continuation arrow sits at one height against the status word, in any font.
 *
 * A font draws its own "↳" at its own height, so the arrow is an SVG. It sits
 * inline in the word, and an inline SVG's baseline is its bottom edge. Sized
 * and lowered in cap units, it runs from the cap line to just below the baseline.
 *
 * Inline is the load-bearing part. As a flex sibling, the row centres it on
 * the word's line box, which lands per font. The row cannot baseline-align
 * either, because `.response-meta > :first-child` owns its centring.
 * jsdom runs no layout, so this pins the wiring as a source scan.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { cssRules } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const chatCss: string = readFileSync(resolve(here, '../chat/input-messages.css'), 'utf8');
const chatExchange: string = readFileSync(
  resolve(here, '../../components/chat/ChatExchange.tsx'),
  'utf8',
);

describe('the continuation arrow spans the cap band', () => {
  it('is an icon, not a font character', () => {
    expect(chatExchange).not.toContain("'↳'");
    expect(chatExchange).toMatch(/<ContinuedIcon className="exchange-status-continued exchange-status-glyph"/);
  });

  it('starts on the cap line and hangs just below the baseline', () => {
    // Sized and lowered in cap units, so the geometry follows the font.
    const rule = cssRules(chatCss).find((r) => r.selector === '.exchange-status-continued');
    expect(rule?.props.get('height')).toBe('calc(1cap * 14 / 12)');
    expect(rule?.props.get('vertical-align')).toBe('calc(-1cap * 2 / 12)');
  });

  it('rides inside the status word, not beside it', () => {
    expect(chatExchange).toMatch(
      /\{statusLabelText\}\s*\{status === 'interrupted' && <ContinuedIcon [^>]*\/>\}\s*<\/span>/,
    );
  });
});
