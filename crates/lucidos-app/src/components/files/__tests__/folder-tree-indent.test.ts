import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';


const here = dirname(fileURLToPath(import.meta.url));
const TREE_TSX = readFileSync(resolve(here, '../FolderTree.tsx'), 'utf8');
const COMPONENTS_CSS = readFileSync(resolve(here, '../../../styles/components.css'), 'utf8');

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

    expect(
      ruleBody('.file-item'),
      'keep the tree file-row offset on .tree-file-item, not on the shared .file-item',
    ).not.toMatch(/padding-left:/);
  });
});
