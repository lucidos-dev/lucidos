import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const drawerCss = readFileSync(resolve(here, '../../../styles/drawer.css'), 'utf-8');
const mobileCss = readFileSync(resolve(here, '../../../styles/mobile.css'), 'utf-8');

/**
 * The title row sits on the page while the transcript rests at its top, and
 * its tinted band fades in once the transcript scrolls. There is no hairline
 * under it on either layout.
 *
 * The band matters most on mobile. The title slides away with the header as
 * one bar. Without a band it drifts like loose transcript text.
 */
describe('Thread title band, desktop and mobile alike', () => {
  it('desktop: page colour at rest, --bg-secondary once scrolled', () => {
    expect(drawerCss).toMatch(/\n\.thread-view-header \{[^}]*background:\s*transparent/);
    expect(drawerCss).toMatch(/\.thread-view-header\.scrolled \{[^}]*background:\s*var\(--bg-secondary\)/);
  });

  it('mobile: page colour at rest, --bg-secondary once scrolled', () => {
    expect(mobileCss).toMatch(/\.mobile-swipe-pane \.mobile-thread-title-row \{[^}]*background:\s*var\(--bg-primary\)/);
    expect(mobileCss).toMatch(/\.mobile-swipe-pane \.mobile-thread-title-row\.scrolled \{[^}]*background:\s*var\(--bg-secondary\)/);
  });

  it('draws no hairline under either row', () => {
    expect(drawerCss).not.toMatch(/\.thread-view-header::after/);
    expect(mobileCss).not.toMatch(/\.mobile-thread-title-row::before/);
  });
});
