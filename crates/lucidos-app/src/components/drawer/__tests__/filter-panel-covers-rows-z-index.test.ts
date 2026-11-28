import { describe, it, expect } from 'vitest';
// @ts-expect-error Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error same
import { dirname, resolve } from 'node:path';
// @ts-expect-error same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const drawerCss: string = readFileSync(resolve(here, '../../../styles/drawer.css'), 'utf-8');

function localZIndexes(css: string): { selector: string; z: number }[] {
  const bare = css.replace(/\/\*[\s\S]*?\*\//g, '');
  const out: { selector: string; z: number }[] = [];
  for (const m of bare.matchAll(/([^{}]+)\{([^}]*)\}/g)) {
    const selector = m[1].trim().replace(/\s+/g, ' ');
    if (selector.startsWith('@')) continue;
    const z = m[2].match(/z-index:\s*(\d+)\s*;/);
    if (!z) continue;
    out.push({ selector, z: parseInt(z[1], 10) });
  }
  return out;
}

describe('the filter panel covers every positioned layer inside the thread list', () => {
  const panel = localZIndexes(drawerCss).find(r => r.selector === '.thread-filter-cover');

  it('declares a plain local z-index', () => {
    expect(panel, '.thread-filter-cover has no plain-number z-index').toBeDefined();
  });

  it('outranks EVERY other local layer in the file, whatever gets added next', () => {
    const others = localZIndexes(drawerCss)
      .filter(r => r.selector !== '.thread-filter-cover' && r.selector !== NAV_COVER);
    if (others.length === 0) return;
    const highest = others.reduce((a, b) => (b.z > a.z ? b : a));
    expect(
      panel!.z,
      `.thread-filter-cover (${panel!.z}) must outrank ${highest.selector} (${highest.z})`,
    ).toBeGreaterThan(highest.z);
  });
});

const NAV_COVER = '.thread-drawer-body > .nav-cover';

describe('the navigation cover lies over the filter panel', () => {
  it('outranks the filter cover', () => {
    const layers = localZIndexes(drawerCss);
    const cover = layers.find(r => r.selector === NAV_COVER);
    const panel = layers.find(r => r.selector === '.thread-filter-cover');
    expect(cover, `${NAV_COVER} has no plain-number z-index`).toBeDefined();
    expect(cover!.z).toBeGreaterThan(panel!.z);
  });
});
