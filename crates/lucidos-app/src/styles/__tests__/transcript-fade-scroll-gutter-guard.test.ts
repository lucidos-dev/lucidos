/**
 * The transcript dissolves at both ends: a mask fades its top under the thread
 * title, and a bg-colored band fades its bottom into the composer. Neither may
 * touch the SCROLLBAR, which is chrome rather than content and has to stay at
 * full opacity whatever the transcript underneath it is doing.
 *
 * Both fades got that wrong, and each for its own reason, which is why this
 * pins both shapes rather than one rule:
 *
 *   - the top fade is a `mask`, and a mask applies to the element's whole
 *     rendering, scrollbar included, so a full-width one dissolved the top of
 *     the thumb (verified in Chromium: a single-layer mask changes the pixels in
 *     the scrollbar strip, the two-layer one below leaves them byte-identical to
 *     no mask at all);
 *   - the bottom fade paints OVER the transcript from `.prompt-area`, so it has
 *     to stop short of the gutter horizontally. It was a box-shadow, which
 *     feathers past the box it is cast from on every side and therefore cannot
 *     be given a clean edge there at any inset.
 *
 * The scroll gutter is `--scrollbar-gutter-width` (0px wherever scrollbars are
 * overlay, which is why neither regression reproduces on a Mac with the default
 * scrollbar setting or on a phone). A source scan rather than a browser test:
 * the regression is about which declaration is written, and the platforms that
 * reserve a classic gutter are exactly the ones our e2e browsers do not emulate.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, decl } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const inputMessagesCss = readFileSync(resolve(here, '../chat/input-messages.css'), 'utf-8');
const contentCss = readFileSync(resolve(here, '../panels/content.css'), 'utf-8');
const responseCss = readFileSync(resolve(here, '../chat/response.css'), 'utf-8');

/** The desktop `.thread-content` rule, i.e. the one that carries the top fade. */
function maskedTranscript(): string {
  const mask = inputMessagesCss.indexOf('-webkit-mask:');
  expect(mask, 'the transcript declares no mask').toBeGreaterThan(0);
  const rule = block(inputMessagesCss, '.thread-content {', inputMessagesCss.lastIndexOf('.thread-content {', mask));
  expect(rule, 'the mask is not on a .thread-content rule').toContain('-webkit-mask:');
  return rule;
}

describe('the transcript fades leave the scroll gutter alone', () => {
  it('keeps the top fade off the scrollbar with a second, opaque mask layer', () => {
    const rule = maskedTranscript();
    for (const prop of ['-webkit-mask', 'mask']) {
      const value = decl(rule, prop);
      expect(value, `${prop} missing`).not.toBeNull();
      // Layer 1 is the fade, narrowed by the gutter; layer 2 is the opaque strip
      // sized to it. Both mentions are load-bearing: without the first the fade
      // still covers the scrollbar, without the second nothing repaints it.
      expect(value).toContain('calc(100% - var(--scrollbar-gutter-width))');
      expect(value).toContain('var(--scrollbar-gutter-width) 100%');
      expect(value).toContain('var(--thread-top-fade)');
    }
  });

  it('never puts a full-width fade back on the transcript', () => {
    // `mask-image` alone cannot express the second layer's size, so a lone
    // gradient there is the single-layer shape this replaced. (Killing the mask
    // outright with `mask-image: none`, as compose-empty does, is fine.)
    expect(maskedTranscript()).not.toMatch(/mask-image:\s*linear-gradient/);
  });

  it('stops the bottom dissolve at the gutter, and sizes it from --prompt-fade', () => {
    const band = block(contentCss, '.prompt-area::before {');
    expect(decl(band, 'left')).toBe('0');
    expect(decl(band, 'right')).toBe('var(--scrollbar-gutter-width)');
    // Sits directly above the composer, exactly as tall as the clearance the
    // transcript's bottom padding reserves for it (--prompt-fade, base.css).
    expect(decl(band, 'bottom')).toBe('100%');
    expect(decl(band, 'height')).toBe('var(--prompt-fade)');
    // A shadow is never hit-tested; this band is an element over the last turn.
    expect(decl(band, 'pointer-events')).toBe('none');
  });

  it('casts no shadow from the composer, which no inset could keep off the gutter', () => {
    expect(decl(block(contentCss, '.prompt-area {'), 'box-shadow')).toBeNull();
  });

  it('parks nothing AT that dissolve: the send landing goes to the top instead', () => {
    // `.response-header` used to declare a `scroll-margin-bottom`, because
    // `landOnOwnTurn` (components/chat/scrollState.ts) rested a turn's agent
    // status line on the transcript's bottom edge, which is under the band above
    // and would have painted over the very row the landing existed to show.
    // A submit lands its turn's TOP on the line at the other end now, so nothing
    // is parked against the dissolve and the clearance has no reader. Pinned so
    // it cannot come back without the landing that needs it.
    expect(decl(block(responseCss, '.response-header {'), 'scroll-margin-bottom')).toBeNull();
  });
});
