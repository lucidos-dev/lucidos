import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const drawerCss = readFileSync(resolve(here, '../../../styles/drawer.css'), 'utf-8');
const mobileCss = readFileSync(resolve(here, '../../../styles/mobile.css'), 'utf-8');

/**
 * Regression: the thread title header divider was missing on mobile.
 *
 * On desktop the title bar is `.thread-view-header` (ThreadView.tsx) and carries
 * its divider as a `::after` 1px hairline in `var(--border-color)`. On mobile the
 * desktop element is `display: none` and the title renders instead in the sticky
 * `.mobile-thread-title-row` (inside the scroll container) — which never got the
 * equivalent hairline. Its only `::after` is the scroll-fade gradient (opacity 0
 * at rest), so at rest there was no divider at all. The two viewports render the
 * title bar with two different elements and only the desktop one had the line.
 *
 * The fix gives `.mobile-thread-title-row` a matching bottom hairline via
 * `::before` (`::after` is taken by the fade gradient), absolutely positioned so
 * it adds no layout height (the `--mobile-thread-title-height` scroll offset is
 * unchanged), always visible like desktop.
 */
describe('Thread title header divider — desktop/mobile parity', () => {
  it('desktop .thread-view-header has a 1px var(--border-color) bottom hairline', () => {
    expect(drawerCss).toMatch(
      /\.thread-view-header::after\s*\{[^}]*bottom:\s*0[^}]*height:\s*1px[^}]*background:\s*var\(--border-color\)/,
    );
  });

  it('mobile .mobile-thread-title-row has a matching 1px var(--border-color) bottom hairline', () => {
    expect(mobileCss).toMatch(
      /\.mobile-thread-title-row::before\s*\{[^}]*bottom:\s*0[^}]*height:\s*1px[^}]*background:\s*var\(--border-color\)/,
    );
  });
});
