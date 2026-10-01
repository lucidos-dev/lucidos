/**
 * Every list panel draws its sections with one header and its rows with one
 * hairline: the thread drawer, Triggers, Changes and Thread queue
 * (docs/plans/2026-09-30-list-panel-section-headers.md).
 *
 * A source scan. The rendered look is measured by
 * e2e/list-panel-sections.spec.ts.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules, selectorList, styleSheetPaths } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(here, '../..');
const read = (path: string): string => readFileSync(resolve(SRC, path), 'utf-8');

const PANELS = {
  triggers: 'components/triggers/TriggerGroupHeader.tsx',
  changes: 'components/changes/ChangesView.tsx',
  threadQueue: 'components/thread-queue/ThreadQueueView.tsx',
};

/** Classes each panel used to style its own header with. */
const RETIRED = [
  'trigger-group-chevron',
  'trigger-group-count',
  'trigger-group-toggle',
  'trigger-group-actions',
  'thread-queue-section-header',
  'thread-queue-section-title',
  'drawer-section-label',
  'drawer-section-icon',
];

describe('the list panels share one section header', () => {
  it('draws every panel header through the shared component', () => {
    for (const [panel, path] of Object.entries(PANELS)) {
      expect(read(path), panel).toContain('<SectionHeader');
    }
    expect(read('components/drawer/ThreadDrawer.tsx')).toContain('<SectionHeaderContent');
  });

  it('leaves no panel its own header class', () => {
    const sources = [
      ...Object.values(PANELS),
      'components/triggers/TriggersView.tsx',
      'components/drawer/ThreadDrawer.tsx',
      'components/layout/ThreadFilterPanel.tsx',
    ].map(read);
    const sheets = styleSheetPaths(resolve(SRC, 'styles')).map((p: string) => readFileSync(p, 'utf-8'));
    // The whole class name, so a spec file named after one does not count.
    const found = RETIRED.filter(cls => {
      const named = new RegExp(`(?<![\\w-])${cls}(?![\\w-])`);
      return [...sources, ...sheets].some(src => named.test(src));
    });
    expect(found).toEqual([]);
  });

  it('keeps the Triggers heading class a hook, with no look of its own', () => {
    // It stays for the hover reveal of rename and delete, and for e2e.
    const skills = cssRules(read('styles/skills.css'));
    const styled = skills.filter(r => selectorList(r.selector).some(s => /\.trigger-group-header\s*$/.test(s)));
    expect(styled.map(r => r.selector)).toEqual([]);
  });

  it('keeps the look host-only, so an app header is unchanged', () => {
    const shared = read('styles/global/shared-components.css');
    expect(shared).not.toContain('list-section-title-collapsible');
    expect(shared).not.toContain('list-rows-divided');
  });
});

describe('the list panels share one row hairline', () => {
  it('marks each panel list as divided', () => {
    expect(read('components/triggers/TriggersView.tsx')).toContain('list-rows list-rows-divided');
    expect(read('components/changes/ChangesView.tsx')).toContain('list-rows-divided');
    expect(read('components/thread-queue/ThreadQueueView.tsx')).toContain('list-rows list-rows-divided');
  });

  it('draws the row line, the drawer row line and a collapsed header line in one rule', () => {
    const rules = cssRules(read('styles/section-header.css'));
    const line = rules.filter(r => r.props.get('border-bottom') === '1px solid var(--border-color)');
    expect(line).toHaveLength(1);
    const selectors = selectorList(line[0].selector);
    expect(selectors).toEqual(expect.arrayContaining([
      '.list-rows-divided .list-row::after',
      '.list-rows-divided .changes-bulk-actions::after',
      '.thread-drawer .thread-row::after',
      '.list-section-title-collapsible.collapsed::after',
    ]));
    expect(line[0].props.get('left')).toBe('var(--row-hairline-left, var(--section-inset-left))');
    expect(line[0].props.get('right')).toBe('var(--row-hairline-right, var(--section-inset-right))');
  });
});
