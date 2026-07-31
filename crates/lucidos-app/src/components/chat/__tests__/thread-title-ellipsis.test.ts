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
 * Regression: a long thread title in the desktop chat header was hard-cut
 * mid-word at the pane's right edge with NO ellipsis.
 *
 * The chain is `.thread-view-header` (row flex) > `.thread-title-edit` (the
 * wrapper, flex:0 1 auto + min-width:0 + overflow:hidden) > the read-only
 * `.thread-title-display` leaf. The culprit was the LEAF: `width: max-content`
 * made its box exactly as wide as its text at every pane width, so the text
 * never overflowed its own box, `text-overflow` never applied, and the WRAPPER
 * did all the truncating with a bare `overflow: hidden` (a hard clip).
 *
 * The fix makes the leaf clip itself: `align-self: stretch` (overriding the
 * wrapper's `align-items: flex-start`) with `width: auto`, so it resolves to
 * the title's own one-line width while it fits and to the shrunken wrapper's
 * width once it doesn't.
 *
 * Both banned shapes below are things that read as correct at a glance:
 *   - `width: max-content` is the original bug (no ellipsis, ever).
 *   - a bare `max-width: 100%` DOES ellipsise, but ~1 char early on every
 *     title, because the leaf carries negative horizontal margins (-0.25rem a
 *     side, cancelling the field's padding) that make the wrapper's content box
 *     0.5rem narrower than the title. Stretch resolves against the same box but
 *     adds the margins back.
 *
 * Layout can't be measured in jsdom, so this pins the CSS shape; the rendered
 * behaviour is covered by e2e/thread-title-resize-desktop.spec.ts.
 */
describe('Desktop chat header title truncates with an ellipsis', () => {
  const leaf = ruleBody(drawerCss, '.thread-view-header .thread-title-input.thread-title-display');
  const wrapper = ruleBody(drawerCss, '.thread-view-header .thread-title-edit');

  it('the display leaf sizes itself off the wrapper, so its text can overflow it', () => {
    expect(leaf).toMatch(/align-self:\s*stretch/);
    expect(leaf).toMatch(/width:\s*auto/);
  });

  it('the display leaf carries the ellipsis and stays on one line', () => {
    expect(leaf).toMatch(/text-overflow:\s*ellipsis/);
    expect(leaf).toMatch(/white-space:\s*nowrap/);
  });

  it('the display leaf is not re-sized to its content (kills text-overflow)', () => {
    expect(leaf).not.toMatch(/width:\s*max-content/);
  });

  it('the display leaf does not cap on a bare percentage (truncates ~1 char early)', () => {
    expect(leaf).not.toMatch(/max-width:\s*100%/);
  });

  it('the wrapper still shrinks below its content so the leaf gets narrowed', () => {
    expect(wrapper).toMatch(/min-width:\s*0/);
    expect(wrapper).toMatch(/overflow:\s*hidden/);
  });
});
