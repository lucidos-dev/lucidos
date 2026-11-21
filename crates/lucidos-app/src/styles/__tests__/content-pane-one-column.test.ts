/**
 * Every content-pane view, tab or overlay, is one column with one left edge,
 * width and top. The column is default-deny, so the ways to lose it are the
 * ones pinned here:
 *
 * - the default-deny rule itself goes missing or narrows to a class list,
 *   which is how the plugin detail page ended up left-aligned;
 * - a view root restates the cap, centres itself or sets its own top spacing,
 *   so it drifts the day the column changes.
 *
 * `e2e/content-pane-one-column-desktop.spec.ts` measures the rendered result.
 * The full-bleed opt-out list is pinned in `content-pane-end-space.test.ts`.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, relative, resolve } from 'node:path';

import { COLUMN_VIEW_ROOTS, cssRules, rulesTargeting, styleSheetPaths } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesRoot: string = resolve(here, '..');
const SHEETS = styleSheetPaths(stylesRoot).map((path: string) => ({ path, css: readFileSync(path, 'utf8') as string }));
const contentCss = cssRules(readFileSync(resolve(stylesRoot, 'panels/content.css'), 'utf8'));

const COLUMN = '.content-pane-body > :not(.content-view-full-bleed)';

/** What places a view root in the pane. The column owns all of it. */
const PLACEMENT = [
  'max-width', 'width', 'max-inline-size', 'inline-size',
  'margin', 'margin-inline', 'margin-left', 'margin-right',
  'margin-top', 'margin-block', 'margin-block-start',
  'padding', 'padding-top', 'padding-block', 'padding-block-start',
];

/** The transcript shares the token, so its cap is the column's twin, not a copy. */
const TRANSCRIPT_SHEETS = /^chat\//;

describe('the content pane column', () => {
  it('is one default-deny rule, capped and centred on the transcript token', () => {
    const rule = contentCss.find(r => r.selector === COLUMN);
    expect(rule, `no rule "${COLUMN}"`).toBeDefined();
    expect(rule!.props.get('max-width')).toBe('var(--content-max-width)');
    expect(rule!.props.get('margin-inline')).toBe('auto');
  });

  it('owns the space above every view', () => {
    const top = contentCss.find(r => r.selector.startsWith(`${COLUMN},`) && r.props.has('padding-top'));
    expect(top?.props.get('padding-top')).toBe('var(--space-md)');
  });

  it('is the only content-pane cap: no other sheet reads the token for a width', () => {
    const caps = SHEETS.flatMap(({ path, css }) => cssRules(css)
      .filter(r => r.props.get('max-width')?.includes('--content-max-width') && r.selector !== COLUMN)
      .map(r => ({ rel: relative(stylesRoot, path) as string, selector: r.selector })))
      .filter(c => !TRANSCRIPT_SHEETS.test(c.rel));
    expect(caps.map(c => `${c.rel}: ${c.selector}`)).toEqual([]);
  });

  it('places every view: no view root sets its own width, margin or top', () => {
    const offenders = SHEETS.flatMap(({ path, css }) => COLUMN_VIEW_ROOTS.flatMap(cls =>
      rulesTargeting(css, cls)
        .filter(r => !r.selector.startsWith(COLUMN) && PLACEMENT.some(p => r.props.has(p)))
        .map(r => `${relative(stylesRoot, path)}: ${r.selector} { ${PLACEMENT.filter(p => r.props.has(p)).join(', ')} }`)));
    expect(offenders, 'let the column place it; a view root may only pad its sides').toEqual([]);
  });
});
