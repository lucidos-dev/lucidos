import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, decl } from '../../../styles/__tests__/css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const styles = (rel: string): string =>
  readFileSync(resolve(here, '../../../styles', rel), 'utf-8');

const baseCss = styles('global/base.css');
const shellCss = styles('panels/shell.css');
const componentsCss = styles('components.css');
const mobileCss = styles('mobile.css');

/**
 * Regression (packaged macOS build): toasts overlapped the header.
 *
 * `.toast-container` is `position: fixed`, so its `top` is measured from the
 * VIEWPORT, and it hardcoded the header's own height (2.75rem plus the
 * safe-area inset). Under the DMG / `.app` build `titleBarStyle: "Overlay"`
 * reclaims the native title bar: `.titlebar-strip` (`--titlebar-inset`, 28px)
 * sits above the header and pushes it down, so the header's bottom edge is 28px
 * lower than the hardcoded value and the toast stack started inside the header.
 *
 * The same bug was fixed once already for the desktop `.drawer-backdrop`, by
 * adding `--titlebar-inset` to a second copy of the same literal. So the fix
 * here is one shared anchor, `--app-header-bottom`, that every fixed element
 * sitting below the header consumes, rather than a third copy of the geometry.
 */
describe('below-header anchor (--app-header-bottom)', () => {
  const rootTokens = block(baseCss, ':root');

  it('base.css defines the header height and the header-bottom anchor', () => {
    expect(decl(rootTokens, '--app-header-height')).not.toBeNull();
    expect(decl(rootTokens, '--app-header-bottom')).not.toBeNull();
  });

  it('the anchor includes the reclaimed macOS title-bar band', () => {
    const anchor = decl(rootTokens, '--app-header-bottom')!;
    expect(anchor).toContain('var(--titlebar-inset');
    expect(anchor).toContain('var(--app-header-height');
  });

  /**
   * The app-shell banner (BackupReminderBanner) is a flow sibling below the
   * desktop header, so it moves the bottom edge of the visible chrome down. The
   * anchor has to carry it, or the toast stack and the drawer backdrop start
   * BEHIND the banner: the same class of bug as the title-bar band above, just
   * from a different term.
   *
   * Mobile needs no term here. Its banner renders inside the fixed .app-header,
   * whose border-box height useHideOnScroll measures into
   * --mobile-header-height, which the mobile anchor already derives from.
   */
  it('the anchor carries the app-shell banner height', () => {
    expect(decl(rootTokens, '--app-banner-height')).not.toBeNull();
    expect(decl(rootTokens, '--app-header-bottom')!).toContain('var(--app-banner-height');
  });

  /**
   * The connection bar is a second banner in that same slot, and both can be up
   * at once. It publishes its OWN property, which the anchor sums: one shared
   * property would mean two ResizeObservers writing one value, so whichever
   * measured last would win and retracting either bar would clear the space the
   * other still occupies.
   */
  it('the anchor carries the connection banner as a second, distinct term', () => {
    expect(decl(rootTokens, '--app-conn-banner-height')).not.toBeNull();
    expect(decl(rootTokens, '--app-header-bottom')!).toContain('var(--app-conn-banner-height');
  });

  it('.app-header sizes itself from the same height token', () => {
    expect(decl(block(shellCss, '.app-header {'), 'height')).toBe('var(--app-header-height)');
  });

  it('the toast stack starts at the header bottom, not a copy of its height', () => {
    const top = decl(block(componentsCss, '.toast-container {'), 'top')!;
    expect(top).toContain('var(--app-header-bottom)');
    // The literal that drifted: the header's own height, restated here.
    expect(top).not.toMatch(/2\.75rem|2\.5rem/);
  });

  it('the desktop drawer backdrop shares the anchor instead of restating it', () => {
    expect(decl(block(mobileCss, '.drawer-backdrop {'), 'top')).toBe('var(--app-header-bottom)');
    const desktop = block(mobileCss, '@media (min-width: 769px)');
    expect(decl(block(desktop, '.drawer-backdrop {'), 'top')).toBeNull();
  });

  it('the mobile layout redefines the anchor from its measured header height', () => {
    // The mobile header is fixed at the viewport top (it overlays the title-bar
    // band rather than sitting below it), so the anchor is its own height,
    // measured live by useHideOnScroll. Must be redefined at the same 768px line
    // the mobile header layout switches at, not the 600px cosmetic breakpoint.
    const mobileRoot = decl(
      block(block(mobileCss, '@media (max-width: 768px)'), ':root'),
      '--app-header-bottom',
    );
    expect(mobileRoot).toContain('var(--mobile-header-height');
  });

  it('no stale copy of the header geometry survives', () => {
    for (const [name, css] of [
      ['components.css', componentsCss],
      ['mobile.css', mobileCss],
    ] as const) {
      expect(css, `${name} restates the header height`).not.toMatch(/2\.5rem \+ 0\.25rem/);
    }
  });
});
