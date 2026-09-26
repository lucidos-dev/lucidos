import { describe, it, expect } from 'vitest';
// @ts-expect-error Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error same
import { dirname, resolve } from 'node:path';
// @ts-expect-error same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const drawerCss: string = readFileSync(resolve(here, '../../../styles/drawer.css'), 'utf-8');

/** Every `z-index: <plain number>` in the file, keyed by the selector list of
 *  the rule it sits in. Deliberately plain integers only: the layers inside the
 *  drawer list are all raw 1-10 values (a component's own stacking context, per
 *  `.claude/rules/frontend-css.md`), and a token-valued one belongs to app
 *  chrome, which is a different question covered by
 *  `ui-blocking-overlay-z-index.test.ts`. */
function localZIndexes(css: string): { selector: string; z: number }[] {
  // Comments first: this file documents its layers heavily, and a `/* … */`
  // before a rule would otherwise land inside the selector capture.
  const bare = css.replace(/\/\*[\s\S]*?\*\//g, '');
  const out: { selector: string; z: number }[] = [];
  for (const m of bare.matchAll(/([^{}]+)\{([^}]*)\}/g)) {
    const selector = m[1].trim().replace(/\s+/g, ' ');
    // An at-rule's own "block" swallows its first nested rule under this flat
    // scan, so skip it rather than attributing a nested layer to `@media`.
    if (selector.startsWith('@')) continue;
    const z = m[2].match(/z-index:\s*(\d+)\s*;/);
    if (!z) continue;
    out.push({ selector, z: parseInt(z[1], 10) });
  }
  return out;
}

/**
 * The filter panel COVERS the thread list inside one pane (`position: absolute;
 * inset: 0` over `.thread-drawer-list`). The trap: `.thread-drawer-list` is NOT
 * a stacking context and cannot become one, because `.flip-portal` is `position:
 * fixed` inside it and has to escape all app chrome to fly a thread between
 * sections. So a ROW's z-index competes directly with the panel's, and "the
 * panel covers the list" is not one layer against one layer.
 */
describe('the filter panel covers every positioned layer inside the thread list', () => {
  const panel = localZIndexes(drawerCss).find(r => r.selector === '.thread-filter-cover');

  it('declares a plain local z-index', () => {
    expect(panel, '.thread-filter-cover has no plain-number z-index').toBeDefined();
  });

  it('outranks EVERY other local layer in the file, whatever gets added next', () => {
    // The general form of the same bug: any new positioned layer on a row would
    // paint over the panel. `.flip-portal` is token-valued, so the plain-number
    // filter above already excludes it. It never crosses the panel anyway: it
    // sits inside the list, which is hidden while the panel is up. The
    // navigation cover is the one layer meant to be above it (next test).
    const others = localZIndexes(drawerCss)
      .filter(r => r.selector !== '.thread-filter-cover' && r.selector !== NAV_COVER);
    if (others.length === 0) return; // Nothing to outrank: vacuously covered.
    const highest = others.reduce((a, b) => (b.z > a.z ? b : a));
    expect(
      panel!.z,
      `.thread-filter-cover (${panel!.z}) must outrank ${highest.selector} (${highest.z})`,
    ).toBeGreaterThan(highest.z);
  });
});

/** The navigation cover hides the swap between the list and the panel, so it
 *  has to paint over both. Under the panel, opening would show the options at
 *  once with the cover fading behind them. */
const NAV_COVER = '.thread-drawer > .nav-cover';

describe('the navigation cover lies over the filter panel', () => {
  it('outranks the filter cover', () => {
    const layers = localZIndexes(drawerCss);
    const cover = layers.find(r => r.selector === NAV_COVER);
    const panel = layers.find(r => r.selector === '.thread-filter-cover');
    expect(cover, `${NAV_COVER} has no plain-number z-index`).toBeDefined();
    expect(cover!.z).toBeGreaterThan(panel!.z);
  });
});
