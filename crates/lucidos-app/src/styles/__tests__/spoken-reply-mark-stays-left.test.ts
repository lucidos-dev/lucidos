import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { block, decl, rulesTargeting } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(resolve(here, '../chat/voice-call.css'), 'utf8');

const WRAPPING = ['wrap', 'wrap-reverse'];

describe('the call mark stays beside the words', () => {
  it('lets no rule wrap the row', () => {
    const wrapping = rulesTargeting(css, 'spoken-reply')
      .filter(rule => WRAPPING.includes(rule.props.get('flex-wrap') ?? ''));
    expect(wrapping.map(rule => `${rule.atRules} ${rule.selector}`)).toEqual([]);
  });

  it('says so, rather than leaving it to the initial value', () => {
    expect(decl(block(css, '.spoken-reply {'), 'flex-wrap')).toBe('nowrap');
  });

  it('lets the bubble shrink instead, so long words still wrap', () => {
    const bubble = block(css, '.spoken-reply-text {');
    expect(decl(bubble, 'flex')).toBe('0 1 auto');
    expect(decl(bubble, 'min-width')).toBe('0');
    expect(decl(bubble, 'overflow-wrap')).toBe('break-word');
  });

  it('keeps the mark slot at one width, so a run shares a left edge', () => {
    const slot = block(css, '.spoken-reply-who {');
    expect(decl(slot, 'width')).toBeTruthy();
    expect(decl(slot, 'flex-shrink')).toBe('0');
  });
});
