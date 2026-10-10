import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';

/**
 * Folder-tree indentation must come from exactly ONE place: the per-level
 * `margin-left` on `.folder-contents`. A folder's children render INSIDE the
 * parent's `.folder-contents`, so the DOM nesting already accumulates the
 * offset — a second, per-row offset stacks on top of it and every level then
 * indents by the running total (0, 1, 3, 6, 10 … rem) instead of one unit each.
 * That triangular compounding is the bug this pins.
 *
 * The test infra has no jsdom, so this is a source scan rather than a render
 * test (precedent: `shared/__tests__/skeleton-guard.test.ts`).
 */

const here = dirname(fileURLToPath(import.meta.url));
const TREE_TSX = readFileSync(resolve(here, '../FolderTree.tsx'), 'utf8');
const COMPONENTS_CSS = readFileSync(resolve(here, '../../../styles/components.css'), 'utf8');

/** Body of the single-class rule `selector` — a bare `.foo { … }` at column 0,
 *  so a `.foo:hover` or nested/indented variant is never picked up instead. */
function ruleBody(selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const match = new RegExp(`(?:^|\\n)${escaped}\\s*\\{([^}]*)\\}`).exec(COMPONENTS_CSS);
  if (!match) throw new Error(`no \`${selector}\` rule in components.css`);
  return match[1];
}

function remValue(body: string, property: string): number | null {
  const match = new RegExp(`${property}:\\s*([\\d.]+)rem`).exec(body);
  return match ? Number(match[1]) : null;
}

describe('folder tree indentation', () => {
  it('carries the per-level offset on .folder-contents', () => {
    const unit = remValue(ruleBody('.folder-contents'), 'margin-left');
    expect(unit, '.folder-contents must carry a rem per-level indent unit').not.toBeNull();
    expect(unit!).toBeGreaterThan(0);
  });

  it('never adds a per-row left offset on top of that nesting', () => {
    expect(
      TREE_TSX,
      'indentation lives on .folder-contents; an inline per-row offset compounds with the DOM nesting',
    ).not.toMatch(/(?:padding|margin)Left/);
    expect(
      TREE_TSX,
      'a per-level indent prop reintroduces the accumulating offset the DOM nesting already provides',
    ).not.toMatch(/\bindent\s*[:=]/);
  });

  it('keeps the tree file-row offset scoped, so flat .file-item lists are unmoved', () => {
    const treeFileOffset = remValue(ruleBody('.tree-file-item'), 'padding-left');
    expect(treeFileOffset, '.tree-file-item must carry the tree file-row offset').not.toBeNull();
    expect(treeFileOffset!).toBeGreaterThan(0);

    // `.file-item` is shared with the flat changed-files list, which has no
    // disclosure arrow to clear — the tree's extra offset must not land there.
    expect(
      ruleBody('.file-item'),
      'keep the tree file-row offset on .tree-file-item, not on the shared .file-item',
    ).not.toMatch(/padding-left:/);
  });
});
