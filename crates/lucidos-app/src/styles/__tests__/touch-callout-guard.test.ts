/**
 * `-webkit-touch-callout: none` suppresses iOS's long-press menu. On an
 * installed iOS PWA that menu is the ONLY per-link escape to a real browser
 * (Open in Safari, alongside Copy Link and Share), and Settings → Links points
 * users at it as the one-off override for the `external_link_target` default.
 *
 * We deliberately do NOT render our own long-press menu: replacing the native
 * one would mean suppressing it, which costs Copy and Share to regain an Open
 * in Safari we already had. That trade only stays correct while nothing
 * suppresses the callout on or above a link.
 *
 * So this is an allowlist, not a ban. Suppression is legitimate on a control
 * that isn't a link (a draggable list row, a button); it is a regression on any
 * ancestor of chat / markdown / preview content. A new entry here means: prove
 * no link lives inside that selector's subtree, then add it with a reason.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync, statSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const STYLES_ROOT = resolve(here, '..');

/** Selector text allowed to suppress the callout, each with why no link is
 *  inside its subtree. Keyed by selector rather than by file so a second rule
 *  added to an already-listed stylesheet is still caught.
 *
 *  Both current entries are `<button>`-shaped: the property INHERITS, so what
 *  matters is the subtree, not the element. An `<a>` inside a `<button>` is
 *  interactive-in-interactive and invalid, which the markdown renderer already
 *  enforces (`inlineLinkStripRenderer` in `utils/renderMarkdown.ts` exists for
 *  exactly that reason), so no link can inherit from these. */
const ALLOWED: ReadonlyArray<readonly [selector: string, why: string]> = [
  ['button, [role="button"], label',
    'form controls. Markdown never emits an anchor inside a button, so nothing inherits it'],
  ['.inline-step',
    'a <button> row that opens the step modal; renders its own text, no anchors'],
  ['.thread-row',
    'thread-drawer rows: whole-row tap targets; ThreadDrawer.tsx renders no anchor'],
];

function cssFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir)) {
    const path = resolve(dir, entry);
    if (statSync(path).isDirectory()) {
      if (entry === '__tests__') continue;
      out.push(...cssFiles(path));
    } else if (path.endsWith('.css')) {
      out.push(path);
    }
  }
  return out;
}

/** Every selector whose block sets `-webkit-touch-callout: none`, normalized to
 *  single-spaced one-liners so formatting churn doesn't churn the allowlist. */
function suppressingSelectors(): Array<{ selector: string; file: string }> {
  const found: Array<{ selector: string; file: string }> = [];
  for (const path of cssFiles(STYLES_ROOT)) {
    const src = readFileSync(path, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
    // Naive block split is enough: these rules are never inside @media in this
    // tree, and a nested-at-rule selector would still surface (just prefixed),
    // which fails closed into the allowlist rather than slipping past.
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
      + 'per-link "Open in Safari" an installed PWA has, which Settings → Links tells users '
      + 'to use. Prove no anchor lives there, then add the selector to ALLOWED with a reason.',
    ).toEqual([]);
  });

  it('keeps the allowlist honest, so a removed rule does not leave a stale entry', () => {
    const present = found.map(f => f.selector);
    const stale = ALLOWED.map(([selector]) => selector).filter(s => !present.includes(s));
    expect(stale, 'allowlisted selector no longer suppresses the callout; drop the entry').toEqual([]);
  });
});
