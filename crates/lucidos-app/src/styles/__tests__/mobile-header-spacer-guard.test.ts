import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, cssRules, decl } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const styles = (rel: string): string => readFileSync(resolve(here, '..', rel), 'utf-8');

const mobileCss = styles('mobile.css');
const drawerCss = styles('drawer.css');

/**
 * Regression: the thread filter panel's top rows rendered behind the fixed
 * mobile header.
 *
 * The mobile header is `position: fixed` over the swipe panes, so a pane's
 * content does not start below it for free: every scroll container a pane
 * shows reserves the header's height itself, through the shared `::before`
 * spacer group in mobile.css. The filter panel shipped missing from that
 * group because it is easy to read as part of the thread list. It is not: it
 * is absolutely positioned OVER the list and scrolls on its own, so it
 * inherits nothing the list reserves.
 *
 * A browser e2e covers the rendered geometry
 * (`e2e/mobile-threads-top-clipping.spec.ts`). This scan covers the same
 * contract in the fast suite, which matters because the symptom is invisible
 * on a desktop viewport and the e2e suite is not what runs per change.
 */
describe('mobile fixed-header spacer', () => {
  /** The rule carrying the `::before` header spacers, found via the list. */
  const spacerRule = cssRules(mobileCss).find(r =>
    r.selector.includes('.thread-drawer-list::before'),
  );

  it('the spacer group exists at the mobile layout line and reserves the header height', () => {
    expect(spacerRule, 'no rule carries .thread-drawer-list::before').toBeDefined();
    expect(spacerRule!.atRules).toContain('@media (max-width: 768px)');
    expect(spacerRule!.props.get('height')).toContain('var(--mobile-header-height');
  });

  /**
   * The load-bearing pair. The panel COVERS the list, so whatever clearance
   * the list needs from the header, the panel needs identically: they occupy
   * the same place on screen. Asserting them together says why, where two
   * separate "is X in the group" assertions would just be a list.
   */
  it('the filter panel reserves it too, since it covers the list', () => {
    for (const subject of ['.thread-drawer-list::before', '.thread-filter-panel::before']) {
      expect(
        spacerRule!.selector,
        `${subject} is not in the mobile header-spacer group, so whatever it shows ` +
        `at the top of the threads pane renders behind the fixed header`,
      ).toContain(subject);
    }
  });

  /**
   * The premise of the test above. If the panel ever renders INSIDE the list
   * instead of over it, it would inherit the list's spacer and its own would
   * become a double gap: this pins the shape the pairing is derived from, so
   * that change fails here rather than silently making the guard wrong.
   *
   * The panel is ONE scroll box, so the `::before` is its first scrolling
   * child, exactly as in the list it covers. It briefly was not: it became a
   * flex column with a pinned Close footer and an inner scrolling body, and
   * this guard was rewritten twice to follow that shape. The footer is gone
   * again (the header's Filter button is the way out, wearing an X while the
   * panel is up), so the scroll is back on the panel and `.thread-filter-panel-body`
   * no longer exists to assert anything about.
   */
  it('the panel really is a cover, not a child of the list', () => {
    const panel = block(drawerCss, '.thread-filter-panel {');
    expect(decl(panel, 'position')).toBe('absolute');
    expect(decl(panel, 'inset')).toBe('0');
    expect(decl(panel, 'overflow-y')).toBe('auto');
  });
});
