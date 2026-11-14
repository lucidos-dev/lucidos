/**
 * A visually hidden label may not scroll the surface it sits in.
 * Plan: docs/plans/2026-09-17-a-hidden-label-stops-inflating-the-transcript.md
 *
 * A source scan, because what it pins is a pair of declarations. What they
 * prevent is measured in the browser by
 * e2e/a-hidden-label-adds-no-scroll.spec.ts, on the mobile projects.
 */

/* `.visually-hidden` hides a label from sight and keeps it in layout, because
 * iOS WebKit drops the `change` event on a file input that is `display: none`.
 * Keeping it in layout is the whole point, and it is also the trap.
 *
 * A zero-width box still LAYS ITS TEXT OUT. With no room the line breaks after
 * every character, so a ten-character label becomes ten line boxes hanging
 * below a box that measures nothing. That column is scrollable overflow.
 * `overflow: visible` carries it up to the nearest scroll container, and no
 * element's rect shows it, which is why three rounds of measuring boxes found
 * nothing.
 *
 * A spoken reply draws one: `SpokenReply` puts "Said aloud" beside the phone
 * glyph. Ten characters at the transcript's line height is 135px, and the LAST
 * spoken row sets the bottom of the scrollable region. So the transcript
 * scrolled 135px past where the conversation ended, and the caller saw a hole
 * between their newest voice message and the composer. Measured on the
 * reporter's own thread at 393x852: a 159.7px gap where the transcript
 * reserves 24.75px.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { rulesTargeting, styleSheetPaths, type CssRule } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesRoot: string = resolve(here, '..');

function rulesFor(className: string): CssRule[] {
  const out: CssRule[] = [];
  for (const path of styleSheetPaths(stylesRoot)) {
    out.push(...rulesTargeting(readFileSync(path, 'utf-8'), className));
  }
  return out;
}

describe('a visually hidden label clips its own text', () => {
  it('is declared once, so there is one place this can regress', () => {
    const declaring = rulesFor('visually-hidden').filter(r => r.props.has('position'));
    expect(declaring.length, 'more than one rule builds the hidden box').toBe(1);
  });

  it('clips what it hides, so the text cannot become scrollable overflow', () => {
    const rule = rulesFor('visually-hidden').find(r => r.props.has('position'));
    expect(rule, 'no rule builds the hidden box at all').toBeDefined();
    expect(
      rule!.props.get('overflow'),
      'the hidden box does not clip: its text spills into the scroll container',
    ).toBe('hidden');
  });

  it('keeps the label on one line, so no column of characters forms', () => {
    const rule = rulesFor('visually-hidden').find(r => r.props.has('position'));
    expect(
      rule!.props.get('white-space'),
      'the label wraps, so a zero-width box stacks it one character per line',
    ).toBe('nowrap');
  });

  it('stays IN layout, which is why it cannot simply be display:none', () => {
    const rule = rulesFor('visually-hidden').find(r => r.props.has('position'));
    // iOS WebKit drops a file input's `change` event when the input is
    // `display: none` or `visibility: hidden`. HiddenFileInput and the
    // composer's live region both depend on this box still existing.
    expect(rule!.props.get('display')).toBeUndefined();
    expect(rule!.props.get('visibility')).toBeUndefined();
    expect(rule!.props.get('position')).toBe('absolute');
  });
});
