import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, cssRules, decl } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const styles = (rel: string): string => readFileSync(resolve(here, '..', rel), 'utf-8');

const mobileCss = styles('mobile.css');
const drawerCss = styles('drawer.css');

describe('mobile fixed-header spacer', () => {
  const spacerRule = cssRules(mobileCss).find(r =>
    r.selector.includes('.thread-drawer-list::before'),
  );

  it('the spacer group exists at the mobile layout line and reserves the header height', () => {
    expect(spacerRule, 'no rule carries .thread-drawer-list::before').toBeDefined();
    expect(spacerRule!.atRules).toContain('@media (--phone-layout)');
    expect(spacerRule!.props.get('height')).toContain('var(--app-header-bottom)');
  });

  it('a pane-filling view takes the pane height less the spacer', () => {
    const fill = cssRules(mobileCss).find(r =>
      r.selector.includes('.content-pane-body > .content-view-full-bleed.active'),
    );
    expect(fill, 'no rule sizes the pane-filling views').toBeDefined();
    expect(fill!.selector).toContain('.content-pane-body > .file-preview-frame');
    expect(fill!.props.get('height')).toBe('calc(100% - var(--app-header-bottom))');
  });

  it('a column view takes it as a floor, never a fixed height', () => {
    const column = cssRules(mobileCss).find(r =>
      r.selector.trim().endsWith('.content-pane-body > .content-view.active'),
    );
    expect(column, 'no rule floors the column views').toBeDefined();
    expect(column!.props.get('min-height')).toBe('calc(100% - var(--app-header-bottom))');
    expect(column!.props.has('height')).toBe(false);
  });

  it('the filter panel reserves it too, since it covers the list', () => {
    for (const subject of ['.thread-drawer-list::before', '.thread-filter-cover::before']) {
      expect(
        spacerRule!.selector,
        `${subject} is not in the mobile header-spacer group, so whatever it shows ` +
        `at the top of the threads pane renders behind the fixed header`,
      ).toContain(subject);
    }
  });

  it('the panel really is a cover, not a child of the list', () => {
    const panel = block(drawerCss, '.thread-filter-cover {');
    expect(decl(panel, 'position')).toBe('absolute');
    expect(decl(panel, 'inset')).toBe('0');
    expect(decl(panel, 'overflow-y')).toBe('scroll');
  });
});
