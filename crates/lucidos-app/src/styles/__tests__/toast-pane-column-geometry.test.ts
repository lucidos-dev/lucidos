/**
 * Geometry half of the "a toast in one pane lowers the other pane's toasts"
 * regression. `toastColumns.ts` decides WHICH stacks exist (covered by its own
 * unit test and by the render test in
 * `components/shared/__tests__/toast-pane-columns.test.tsx`); this pins the CSS
 * that makes those stacks independent.
 *
 * The bug was structural: one flex column held every toast, and a per-toast
 * `--toast-shift` nudged each one sideways over its pane. Sideways was all it
 * did, so a thread-pane toast still consumed a row of the shared column and
 * pushed the content pane's toasts down. Stacking therefore has to live on the
 * per-pane column, never back on the container.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const componentsCss = readFileSync(resolve(here, '../components.css'), 'utf-8');
const shellCss = readFileSync(resolve(here, '../panels/shell.css'), 'utf-8');

/** Body of the first rule block whose header matches `needle`, from `from`. */
function block(css: string, needle: string, from = 0): string {
  const at = css.indexOf(needle, from);
  expect(at, `"${needle}" not found`).toBeGreaterThanOrEqual(0);
  const open = css.indexOf('{', at);
  let depth = 0;
  for (let i = open; i < css.length; i++) {
    if (css[i] === '{') depth++;
    else if (css[i] === '}' && --depth === 0) return css.slice(open + 1, i);
  }
  throw new Error(`unterminated block for "${needle}"`);
}

/** A declaration's value, or null when the block doesn't set that property. */
function decl(body: string, prop: string): string | null {
  const m = body.match(new RegExp(`(?:^|[;{]|\\*/)\\s*${prop}:\\s*([^;]+);`));
  return m ? m[1].replace(/\s+/g, ' ').trim() : null;
}

describe('per-pane toast columns', () => {
  it('stacks toasts on the column, not on the shared container', () => {
    const column = block(componentsCss, '.toast-column {');
    expect(decl(column, 'display')).toBe('flex');
    expect(decl(column, 'flex-direction')).toBe('column');
    expect(decl(column, 'gap')).not.toBeNull();

    // The container only positions the columns. A flex column here would put
    // every toast back into one stack, which is the bug.
    const container = block(componentsCss, '.toast-container {');
    expect(decl(container, 'flex-direction')).toBeNull();
    expect(decl(container, 'gap')).toBeNull();
  });

  it('gives each pane its own column, spanning that pane', () => {
    const desktop = componentsCss.indexOf('@media (min-width: 769px)', componentsCss.indexOf('.toast-container {'));
    expect(desktop).toBeGreaterThan(0);
    expect(decl(block(componentsCss, '.toast-column {', desktop), 'position')).toBe('absolute');

    // The shared header geometry (shell.css): drawer width, drawer-divider
    // offset, split position, divider width. Same segmentation as the header
    // focus wash, so a column always covers exactly its pane.
    const thread = block(componentsCss, '.toast-column[data-toast-pane="thread"] {');
    expect(decl(thread, 'left')).toBe('calc(var(--co) + var(--ddo))');
    expect(decl(thread, 'width')).toBe('calc(var(--divider-x) - var(--co) - var(--ddo))');

    const content = block(componentsCss, '.toast-column[data-toast-pane="content"] {');
    expect(decl(content, 'left')).toBe('calc(var(--divider-x) + var(--divider-width))');
    expect(decl(content, 'width')).toBe('calc(100% - var(--divider-x) - var(--divider-width))');
  });

  it('drops the shared-column era per-toast horizontal nudge', () => {
    expect(componentsCss).not.toContain('--toast-shift');
    // Collapse is handled in JS by merging the stacks (toastColumns.ts), so no
    // rule may re-point a column at the other pane's geometry.
    expect(componentsCss).not.toMatch(/data-(thread|content)-collapsed\][^{]*\.toast/);
  });

  it('tracks the panes 1:1 during a divider drag', () => {
    // The column eases its geometry like the header regions, so it must join
    // their resize kill list or it visibly lags the pointer mid-drag.
    expect(decl(block(componentsCss, '.toast-column {', componentsCss.indexOf('@media (min-width: 769px)', componentsCss.indexOf('.toast-container {'))), 'transition'))
      .toContain('var(--duration-slow)');
    expect(shellCss).toContain(':root[data-pane-resizing] .toast-column,');
  });
});
