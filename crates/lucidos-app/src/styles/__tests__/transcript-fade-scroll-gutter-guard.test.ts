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
      expect(value).toContain('calc(100% - var(--scrollbar-gutter-width))');
      expect(value).toContain('var(--scrollbar-gutter-width) 100%');
      expect(value).toContain('var(--thread-top-fade)');
    }
  });

  it('never puts a full-width fade back on the transcript', () => {
    expect(maskedTranscript()).not.toMatch(/mask-image:\s*linear-gradient/);
  });

  it('drops the top fade at rest at the top, where it could only dissolve the first turn\'s marker', () => {
    const atRest = block(
      inputMessagesCss,
      '.thread-view-header:not(.scrolled) + .thread-content-wrap > .thread-content {',
    );
    expect(decl(atRest, '--thread-top-fade-floor')).toBe('1');

    const rule = maskedTranscript();
    for (const prop of ['-webkit-mask', 'mask']) {
      expect(decl(rule, prop)).toContain('rgb(0 0 0 / var(--thread-top-fade-floor, 0)) 0');
    }
    expect(decl(rule, 'transition')).toBe('--thread-top-fade-floor var(--duration-normal) ease');
    expect(inputMessagesCss).toMatch(/@property --thread-top-fade-floor\s*\{[^}]*syntax: '<number>'/);
  });

  it('stops the bottom dissolve at the gutter, and sizes it from --prompt-fade', () => {
    const band = block(contentCss, '.prompt-area::before {');
    expect(decl(band, 'left')).toBe('0');
    expect(decl(band, 'right')).toBe('var(--scrollbar-gutter-width)');
    expect(decl(band, 'bottom')).toBe('100%');
    expect(decl(band, 'height')).toBe('var(--prompt-fade)');
    expect(decl(band, 'pointer-events')).toBe('none');
  });

  it('casts no shadow from the composer, which no inset could keep off the gutter', () => {
    expect(decl(block(contentCss, '.prompt-area {'), 'box-shadow')).toBeNull();
  });

  it('parks nothing AT that dissolve: the send landing clears it by the padding', () => {
    expect(decl(block(responseCss, '.response-header {'), 'scroll-margin-bottom')).toBeNull();
  });
});
