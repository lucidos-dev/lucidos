/**
 * The changed-files list beside a diff: its rows and the sidebar that holds it.
 *
 * A long path wraps the row onto several lines. The icon, diffstat and badge
 * then hold the top and share a line with the path's first line. The sidebar
 * wears the pane's own background, the surface the list has when it stands
 * alone.
 *
 * A source scan, like the sibling geometry guards: what regresses is which
 * declaration is written.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

import { rulesTargeting } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const COMPONENTS: string = readFileSync(resolve(here, '../components.css'), 'utf8');
const CONTENT: string = readFileSync(resolve(here, '../panels/content.css'), 'utf8');

function soleRule(css: string, className: string, selector: string) {
  const rules = rulesTargeting(css, className).filter(r => r.selector === selector);
  expect(rules, `${selector} must be declared exactly once`).toHaveLength(1);
  return rules[0].props;
}

describe('a changed-files row', () => {
  const row = soleRule(COMPONENTS, 'repo-changed-file', '.file-item.repo-changed-file');

  it('holds its icon, diffstat and badge at the top', () => {
    expect(row.get('align-items')).toBe('flex-start');
  });

  it('gives each line the icon box height, so they share the first line', () => {
    const icon = soleRule(COMPONENTS, 'file-icon', '.file-icon');
    expect(row.get('line-height')).toBe(icon.get('height'));
  });
});

describe('the changed-files sidebar', () => {
  it('paints no fill of its own over the pane background', () => {
    const sidebar = soleRule(CONTENT, 'repo-preview-split-sidebar', '.repo-preview-split-sidebar');
    expect(sidebar.has('background')).toBe(false);
    expect(sidebar.has('background-color')).toBe(false);
  });
});
