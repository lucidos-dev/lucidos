import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const drawerCss = readFileSync(resolve(here, '../../../styles/drawer.css'), 'utf-8');

/** Body of the first rule whose selector list matches `selector` exactly. */
function ruleBody(css: string, selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const m = css.match(new RegExp(`(^|})\\s*${escaped}\\s*\\{([^}]*)\\}`, 'm'));
  if (!m) throw new Error(`no rule for selector: ${selector}`);
  return m[2];
}

/**
 * Regression: a long thread title in the desktop header was hard-cut mid-word
 * at the pane's right edge with NO ellipsis.
 *
 * The title is one flex item of `.thread-view-header`. It must shrink below its
 * text (`min-width: 0` with a shrinking `flex`), so its own box gets narrower
 * than the text and `text-overflow` applies. A `width: max-content` box is
 * always exactly as wide as its text, so the ellipsis never fires.
 *
 * Layout can't be measured in jsdom, so this pins the CSS shape; the rendered
 * behaviour is covered by e2e/thread-title-resize-desktop.spec.ts.
 */
describe('Desktop header title truncates with an ellipsis', () => {
  const base = ruleBody(drawerCss, '.thread-title');
  const desktop = ruleBody(drawerCss, '.thread-view-header .thread-title');

  it('shrinks below its text, so the text can overflow it', () => {
    expect(base).toMatch(/min-width:\s*0/);
    expect(desktop).toMatch(/flex:\s*0 1 auto/);
    expect(desktop).not.toMatch(/width:\s*max-content/);
  });

  it('carries the ellipsis and stays on one line', () => {
    expect(desktop).toMatch(/text-overflow:\s*ellipsis/);
    expect(desktop).toMatch(/white-space:\s*nowrap/);
    expect(desktop).toMatch(/overflow:\s*hidden/);
  });
});
