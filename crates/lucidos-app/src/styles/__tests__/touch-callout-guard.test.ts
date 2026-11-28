import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';

import { styleSheetPaths } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const STYLES_ROOT = resolve(here, '..');

const ALLOWED: ReadonlyArray<readonly [selector: string, why: string]> = [
  ['button, [role="button"], label',
    'form controls. Markdown never emits an anchor inside a button, so nothing inherits it'],
  ['.inline-step',
    'a <button> row that opens the step modal; renders its own text, no anchors'],
  ['.thread-row',
    'thread-drawer rows: whole-row tap targets; ThreadDrawer.tsx renders no anchor'],
  ['.app-row',
    'apps panel rows: whole-row tap targets with a long-press menu; AppCard.tsx renders no anchor'],
  ['.ladder-row',
    'intelligence ladder rows, compact layout only: press and drag reorders; renders text, buttons and a switch, no anchors'],
];

function suppressingSelectors(): Array<{ selector: string; file: string }> {
  const found: Array<{ selector: string; file: string }> = [];
  for (const path of styleSheetPaths(STYLES_ROOT)) {
    const src = readFileSync(path, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
    for (const block of src.split('}')) {
      const brace = block.indexOf('{');
      if (brace === -1) continue;
      if (!/-webkit-touch-callout\s*:\s*none/.test(block.slice(brace))) continue;
      found.push({
        selector: block.slice(0, brace).replace(/\s+/g, ' ').trim(),
        file: relative(STYLES_ROOT, path),
      });
    }
  }
  return found;
}

describe('iOS long-press callout suppression', () => {
  const found = suppressingSelectors();

  it('is confined to the reviewed allowlist', () => {
    const allowed = ALLOWED.map(([selector]) => selector);
    const unexpected = found.filter(f => !allowed.includes(f.selector))
      .map(f => `${f.file}: ${f.selector}`);
    expect(
      unexpected,
      'A new `-webkit-touch-callout: none` suppresses iOS\'s long-press menu, and the '
      + 'property INHERITS. If any link can appear in that subtree, this removes the only '
      + 'per-link "Open in Safari" an installed PWA has, which Settings → Appearance & Behavior → Links tells users '
      + 'to use. Prove no anchor lives there, then add the selector to ALLOWED with a reason.',
    ).toEqual([]);
  });

  it('gives the file preview image its menu back, though it wears role="button"', () => {
    const component = readFileSync(resolve(STYLES_ROOT, '../components/files/PreviewImage.tsx'), 'utf8');
    expect(component).toMatch(/class="preview-image"[\s\S]*role="button"/);
    const css = readFileSync(resolve(STYLES_ROOT, 'panels/previews.css'), 'utf8');
    expect(css).toMatch(/img\.preview-image\s*\{[^}]*-webkit-touch-callout:\s*default/);
  });

  it('keeps the allowlist honest, so a removed rule does not leave a stale entry', () => {
    const present = found.map(f => f.selector);
    const stale = ALLOWED.map(([selector]) => selector).filter(s => !present.includes(s));
    expect(stale, 'allowlisted selector no longer suppresses the callout; drop the entry').toEqual([]);
  });
});
