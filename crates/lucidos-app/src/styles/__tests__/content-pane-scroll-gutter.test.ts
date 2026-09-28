/**
 * A content-pane column keeps one width whether or not it overflows. With
 * classic scrollbars, a view growing past one screen otherwise gains a
 * scrollbar and slides sideways. A browser with overlay scrollbars cannot show
 * the slide, so the source pins it too.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

import { cssRules } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const RULES = cssRules(readFileSync(resolve(here, '../panels/shell.css'), 'utf8'));
const COLUMN = '.content-pane-body:has(> .content-view.active:not(.content-view-full-bleed), > .panel-content)';

function prop(selector: string, name: string): string | undefined {
  return RULES.find(r => r.selector === selector && r.props.has(name))?.props.get(name);
}

describe('the content pane scroll gutter', () => {
  it('is reserved for a column view in every engine', () => {
    expect(prop(COLUMN, 'scrollbar-gutter')).toBe('stable');
    expect(prop(COLUMN, 'overflow-y')).toBe('scroll');
  });

  it('is left off a full-bleed view, which keeps its right edge', () => {
    expect(prop('.content-pane-body', 'overflow-y')).toBe('auto');
    expect(prop('.content-pane-body', 'scrollbar-gutter')).toBeUndefined();
  });
});
