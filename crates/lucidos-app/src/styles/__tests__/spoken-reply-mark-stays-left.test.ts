/**
 * The call mark sits to the LEFT of a spoken reply, at every reply length.
 *
 * The row was a wrapping flex container, and a flex line is collected from
 * each item's UNWRAPPED width. So a reply wider than the pane opened a second
 * line and left the mark alone on the first, above the bubble. A short reply
 * fitted and kept the mark beside the words, so one call drew the mark in two
 * different places.
 *
 * Nothing else in the gate can see this. `tsc` skips CSS and `vite build`
 * fails only on a syntax error, so a reintroduced wrap would ship silently.
 */
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

/** Values that would put the bubble on a line of its own. */
const WRAPPING = ['wrap', 'wrap-reverse'];

describe('the call mark stays beside the words', () => {
  it('lets no rule wrap the row', () => {
    const wrapping = rulesTargeting(css, 'spoken-reply')
      .filter(rule => WRAPPING.includes(rule.props.get('flex-wrap') ?? ''));
    expect(wrapping.map(rule => `${rule.atRules} ${rule.selector}`)).toEqual([]);
  });

  it('says so, rather than leaving it to the initial value', () => {
    // Stated, because `nowrap` IS the initial value and a silent default reads
    // as nobody having decided. The rule above it says why it matters.
    expect(decl(block(css, '.spoken-reply {'), 'flex-wrap')).toBe('nowrap');
  });

  it('lets the bubble shrink instead, so long words still wrap', () => {
    // The row cannot wrap now, so this pair is what keeps a long reply inside
    // the pane: the bubble gives up width and breaks its own text.
    const bubble = block(css, '.spoken-reply-text {');
    expect(decl(bubble, 'flex')).toBe('0 1 auto');
    expect(decl(bubble, 'min-width')).toBe('0');
    expect(decl(bubble, 'overflow-wrap')).toBe('break-word');
  });

  it('keeps the mark slot at one width, so a run shares a left edge', () => {
    // The mark is drawn once per run and the slot is kept either way. A slot
    // that collapsed would step every later bubble of the run to the left.
    const slot = block(css, '.spoken-reply-who {');
    expect(decl(slot, 'width')).toBeTruthy();
    expect(decl(slot, 'flex-shrink')).toBe('0');
  });
});
