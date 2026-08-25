/**
 * The drawer row's status mark is centered on the title's FIRST line. It is that
 * line's box, rather than a constant that looks like it.
 *
 * The dot, spinner, "?" badge and pause glyph all hang off `.thread-row-wrap`,
 * beside the row rather than inside it, which takes them out of its flow. For
 * years their vertical place was a `top` literal,
 * hand-tuned per breakpoint against whatever `line-height: normal` resolved to
 * for Fira Code. It read a third of a line low on every row, and it had already
 * been re-tuned twice. It drifted further on any other `--font-ui` the user
 * picked, because `normal` is font metrics.
 *
 * So the title states its line height and the mark's box takes it: top at the
 * row's own padding, one line tall, centered by the flex rule. Nothing here is
 * checkable by the rest of the gate. `tsc` skips CSS and `vite build` only fails
 * on syntax, so a reintroduced literal would ship silently.
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
const drawerCss = readFileSync(resolve(here, '../drawer.css'), 'utf8');

/** The custom property naming one line of the row title. */
const LINE = '--thread-row-title-line';
/** The custom property naming a row's per-depth indent. */
const DEPTH = '--thread-depth-offset';

describe('drawer status dot rides the title line', () => {
  it('states the title line height instead of leaving it to font metrics', () => {
    expect(decl(block(drawerCss, '.thread-row-wrap {'), LINE)).toBeTruthy();
    expect(decl(block(drawerCss, '.thread-row-title-row {'), 'line-height'))
      .toBe(`var(${LINE})`);
  });

  it('sizes the mark box to that line, at the row padding', () => {
    const body = block(drawerCss, '.thread-row-wrap > .thread-status {');
    // `--space-sm` IS `.list-row`'s padding-top, so the box starts where the
    // title's first line does. A literal here would be the old bug's shape.
    expect(decl(body, 'top')).toBe('var(--space-sm)');
    expect(decl(body, 'height')).toBe(`var(${LINE})`);
  });

  it('indents the mark by the same depth step the title takes', () => {
    // One definition of the step, read by the row's padding and by the mark.
    // Two copies of `depth * 1rem` would let a sub-thread's mark drift away
    // from the title it belongs to the next time either side is re-tuned.
    expect(decl(block(drawerCss, '.thread-row-wrap {'), DEPTH)).toBeTruthy();
    for (const [needle, prop] of [['.thread-row {', '--thread-row-pad-left'], ['.thread-row-wrap > .thread-status {', 'left']] as const) {
      expect(decl(block(drawerCss, needle), prop)).toContain(`var(${DEPTH}, 0px)`);
    }
  });

  it('lets no other rule nudge the mark off that line', () => {
    // The old per-breakpoint overrides lived in `@media` blocks below the base
    // rule, where a first-textual-match read would never have seen them.
    const offenders = rulesTargeting(drawerCss, 'thread-status')
      .filter(r => r.props.has('top') || r.props.has('height') || r.props.has('transform'))
      .filter(r => r.selector !== '.thread-row-wrap > .thread-status' || r.atRules !== '');
    expect(offenders.map(r => `${r.atRules} ${r.selector}`)).toEqual([]);
  });

  it('keeps the draft chip inside the line it sits on', () => {
    // An inline-block inheriting the stated line height as its content height,
    // then adding padding, grows the line box and slides the title off the mark.
    expect(decl(block(drawerCss, '.draft-indicator {'), 'line-height')).toBeTruthy();
  });
});
