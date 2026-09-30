/**
 * The drawer row's status mark starts the title's first line and centers on
 * the title's capital letters, in whatever font the theme picks.
 *
 * The mark flows inline before the title, so a wrapped title continues under
 * it and the row's left margin stays free. A mark positioned by `top` or
 * `left` literals drifts per font and reserves a column the title cannot use.
 * Centering on the line box also drifts: it reads about 2px low on a phone.
 * Nothing here is checkable by the rest of the gate: `tsc` skips CSS and
 * `vite build` only fails on syntax.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { block, cssRules, decl, rulesTargeting, selectorList } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const drawerCss = readFileSync(resolve(here, '../drawer.css'), 'utf8');
const drawerTsx = readFileSync(resolve(here, '../../components/drawer/ThreadDrawer.tsx'), 'utf8');
const iconsTsx = readFileSync(resolve(here, '../../components/shared/icons.tsx'), 'utf8');

/** The custom property naming one line of the row title. */
const LINE = '--thread-row-title-line';
const MARK = '.thread-row-title-text > .thread-status';
const ACTIONS = '.thread-row-actions';

/** Declarations of the one rule that centers both the mark and the actions. */
function capCentringRule(): Map<string, string> {
  const rules = cssRules(drawerCss).filter(r => selectorList(r.selector).includes(MARK));
  expect(rules.length).toBeGreaterThan(0);
  const shared = rules.find(r => selectorList(r.selector).includes(ACTIONS));
  expect(shared, `no rule names both ${MARK} and ${ACTIONS}`).toBeTruthy();
  return shared!.props;
}

describe('drawer status mark starts the title line', () => {
  it('states the title line height instead of leaving it to font metrics', () => {
    expect(decl(block(drawerCss, '.thread-row {'), LINE)).toBeTruthy();
    expect(decl(block(drawerCss, '.thread-row-title-row {'), 'line-height'))
      .toBe(`var(${LINE})`);
  });

  it('flows inline, centered on half a cap height above the baseline', () => {
    // `middle` puts the box's center half an x-height up; the bottom margin
    // of `1cap - 1ex` lifts it to half a cap height. A zero-tall box keeps
    // the drawn mark from growing the line.
    const props = capCentringRule();
    expect(props.get('display')).toBe('inline-flex');
    expect(props.get('vertical-align')).toBe('middle');
    expect(props.get('height')).toBe('0');
    expect(props.get('margin-bottom')).toBe('calc(1cap - 1ex)');
  });

  it('measures its ex and cap on the title font size', () => {
    // The units resolve against the mark's own font, inherited from the row.
    // A size set only on the title would size the letters and not the units.
    expect(decl(block(drawerCss, '.thread-row-title-row {'), 'font-size')).toBe('var(--font-size-md)');
    expect(decl(block(drawerCss, '.thread-row-title {'), 'font-size')).toBeFalsy();
  });

  it('renders inside the title text, before the title, in both row kinds', () => {
    // Thread rows and draft rows each draw the mark. Anywhere else the mark
    // rules do not match it, so it would sit on a line of its own.
    const rows = drawerTsx.split('<span class="thread-row-title-text">').slice(1);
    expect(rows.length).toBe(2);
    for (const row of rows) {
      expect(row.trimStart().startsWith('<ThreadStatusIcon')).toBe(true);
    }
    expect(drawerTsx.match(/<ThreadStatusIcon/g)?.length).toBe(2);
  });

  it('takes no room on an idle row', () => {
    const idle = cssRules(drawerCss).find(r => selectorList(r.selector).includes('.thread-row-title-text > .thread-status-idle'));
    expect(idle?.props.get('display')).toBe('none');
  });

  it('draws the thread pane title\'s mark the same way, so the two read alike', () => {
    const shared = cssRules(drawerCss).find(r => selectorList(r.selector).includes(MARK));
    expect(selectorList(shared!.selector)).toContain('.thread-title > .thread-status');
    const idle = cssRules(drawerCss).find(r => selectorList(r.selector).includes('.thread-title > .thread-status-idle'));
    expect(idle?.props.get('display')).toBe('none');
  });

  it('centers the row actions on the same cap height as the mark', () => {
    // The buttons are taller than a line. A box whose top meets the line's
    // top puts the pin below the title's middle. So the actions take the
    // mark's zero-tall inline box, and no other rule may re-flex them.
    capCentringRule();
    const offenders = rulesTargeting(drawerCss, 'thread-row-actions')
      .filter(r => ['display', 'height', 'margin-bottom', 'margin-top', 'position'].some(p => r.props.has(p)))
      .filter(r => !selectorList(r.selector).includes(MARK));
    expect(offenders.map(r => `${r.atRules} ${r.selector}`)).toEqual([]);
  });

  it('centers the pin by its body, not by its box', () => {
    // With the stroke, the pin's head and body span 1 to 18 of its 24-unit
    // box, and the needle hangs below. The eye reads the body, whose center
    // sits 2.5 units above the box's. A new glyph needs a new offset.
    expect(iconsTsx).toContain('<path d="M12 17v5" />');
    expect(decl(block(drawerCss, '.thread-row-actions .pin-thread-btn svg {'), 'translate'))
      .toBe('0 calc(100% * 2.5 / 24)');
  });

  it('gives the row actions a title line of their own to sit in', () => {
    // An inline box needs a line box around it. It inherits the title row's
    // font and line height, so it sits on the title's own baseline.
    const rows = drawerTsx.split('<span class="thread-row-actions">').slice(1);
    expect(rows.length).toBe(1);
    expect(drawerTsx).toContain('<span class="thread-row-actions-line"><span class="thread-row-actions">');
    const line = block(drawerCss, '.thread-row-actions-line {');
    expect(decl(line, 'line-height')).toBeNull();
    expect(decl(line, 'font-size')).toBeNull();
  });

  it('lets no rule pull the mark out of the flow', () => {
    const offenders = rulesTargeting(drawerCss, 'thread-status')
      .filter(r => ['position', 'top', 'left', 'transform'].some(p => r.props.has(p)));
    expect(offenders.map(r => `${r.atRules} ${r.selector}`)).toEqual([]);
  });

  it('keeps the draft chip inside the line it sits on', () => {
    // An inline-block inheriting the stated line height as its content height,
    // then adding padding, grows the line box and slides the title off the mark.
    expect(decl(block(drawerCss, '.draft-indicator {'), 'line-height')).toBeTruthy();
  });
});
