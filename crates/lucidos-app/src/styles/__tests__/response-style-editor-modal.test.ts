/**
 * The response style editor's modal has to show as much of an instruction as
 * the screen allows, and let the user scroll the rest.
 *
 * Both halves were reported from a phone. The form sat in the leftover half of
 * a settings row, too narrow to read. Its field also grew to its own content,
 * so there was no height left to scroll the rest of a long one in.
 *
 * Scanned rather than measured in a browser: each failure is a property of the
 * rule, and shows up only at one viewport with one long instruction.
 * `rulesTargeting` is the reader, so a later sheet re-raising one of these is
 * caught rather than passed over by a first textual match.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, decl, rulesTargeting, styleSheetPaths } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const css: string = readFileSync(resolve(here, '../settings/response-style-editor.css'), 'utf-8');
const everySheet: string[] = styleSheetPaths(resolve(here, '..'))
  .map((p: string) => readFileSync(p, 'utf-8'));

describe('the response style modal shows as much instruction as it can', () => {
  it('takes all the width bar a single gutter', () => {
    // `min(<absolute>, <viewport>)`: a column on a desktop, and the whole
    // screen on a phone. A fixed rem width alone is what left a
    // thousand-character instruction in a slot too narrow to read.
    const width = decl(block(css, '.style-editor-modal {'), 'width');
    expect(width).not.toBeNull();
    expect(width!.startsWith('min(')).toBe(true);
    expect(width).toContain('100vw');
  });

  it('is imported, so every rule here actually ships', () => {
    const entry: string = readFileSync(resolve(here, '../settings.css'), 'utf-8');
    expect(entry).toContain("@import './settings/response-style-editor.css';");
  });

  /** A textarea's intrinsic height is its `rows` attribute, never its content.
   *  So a panel that sizes to its content gives the field its minimum and
   *  leaves the rest of the screen empty. A definite height is what hands the
   *  leftover over. */
  it('gives the panel a definite height, bounded by the VISIBLE viewport', () => {
    const panel = block(css, '.style-editor-modal {');
    // `--app-height` is the visual viewport. Raw `100dvh` does not shrink for
    // the iOS keyboard, which would put Save under the keys.
    expect(decl(panel, '--style-editor-room')).toContain('--app-height');
    expect(decl(panel, 'height')).toContain('var(--style-editor-room)');
    expect(decl(panel, 'max-height'), 'a max-height sizes to content instead')
      .toBeNull();

    // And the scrim it is centred in is bounded the same way, or the panel is
    // centred in a viewport half of which is keyboard.
    expect(decl(block(css, '.modal-overlay.style-editor-overlay {'), 'height'))
      .toContain('--app-height');
  });

  /** The desktop ceiling is the one number a phone must not inherit: a narrow
   *  line holds about a third of the characters, so the same instruction needs
   *  three times the lines to show. */
  it('drops the desktop ceiling on a phone, leaving the screen as the only bound', () => {
    const mobile = rulesTargeting(css, 'style-editor-modal')
      .filter((rule) => rule.atRules.startsWith('@media'));

    expect(mobile, 'no mobile rule releases the ceiling').toHaveLength(1);
    expect(mobile[0].props.get('height')).toBe('var(--style-editor-room)');
  });

  it('hands that leftover height to the instruction, which scrolls inside it', () => {
    const field = block(css, '.style-editor-instruction {');
    expect(decl(field, 'overflow-y')).toBe('auto');
    // A drag handle is unreachable on a phone, and the box is already as tall
    // as the viewport allows.
    expect(decl(field, 'resize')).toBe('none');

    // `min-height: 0` on the wrapper is what lets the flex item shrink below
    // its content. Without it the field takes its content height, the panel
    // overflows, and the cap clips the text instead of scrolling it.
    const grow = block(css, '.style-editor-field-grow {');
    expect(decl(grow, 'min-height')).toBe('0');
    expect(decl(grow, 'flex')).toBe('1 1 auto');

    const panel = block(css, '.style-editor-modal {');
    expect(decl(panel, 'display')).toBe('flex');
    expect(decl(panel, 'flex-direction')).toBe('column');
  });

  /** A phone in landscape with the keyboard up leaves about 190px of visual
   *  viewport, which is under the panel's own squeezed layout. `overflow:
   *  hidden` clipped Save off the bottom there, with no way to reach it. */
  it('keeps Save reachable on a screen too short for the layout', () => {
    const panel = block(css, '.style-editor-modal {');
    expect(decl(panel, 'overflow-y'), 'a clipped panel loses its buttons')
      .toBe('auto');
    expect(decl(panel, 'overflow'), 'a shorthand would clip the other axis')
      .toBeNull();

    // Only the instruction gives up height. A squeezed paragraph spills its
    // text, and a squeezed action row is the one thing that must stay put.
    expect(decl(block(css, '.style-editor-modal > * {'), 'flex-shrink')).toBe('0');
    // And the one child that reopens it has to come AFTER, being equally
    // specific: source order is all that decides between the two.
    expect(css.indexOf('.style-editor-modal > *'))
      .toBeLessThan(css.indexOf('.style-editor-field-grow {'));
  });

  it('is not un-scrolled by any later sheet', () => {
    const raisers = everySheet
      .flatMap((sheet: string) => rulesTargeting(sheet, 'style-editor-instruction'))
      .filter((rule) => ['overflow', 'overflow-y'].some((p) => rule.props.has(p)))
      .filter((rule) => rule.props.get('overflow-y') !== 'auto');

    expect(
      raisers.map((r) => `${r.atRules} ${r.selector} { ${r.body} }`),
      'a later rule takes the scroll off the instruction field',
    ).toEqual([]);
  });
});
