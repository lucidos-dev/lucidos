/**
 * Every content-pane view keeps the end space below its last element
 * (`docs/glossary.md` § End space). The rule is default-deny, so the two ways
 * to lose it are the ones pinned here:
 *
 * - a view pinned to the pane height, whose content then overflows its own box
 *   and leaves the spacer stranded at the pane's bottom edge;
 * - a new full-bleed opt-out nobody reviewed.
 *
 * `e2e/content-pane-end-space.spec.ts` measures the rendered result.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, relative, resolve } from 'node:path';

import { clientSourcePaths, cssRules, selectorList, styleSheetPaths } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesRoot: string = resolve(here, '..');
const srcRoot: string = resolve(here, '../..');
const sheet = (rel: string) => cssRules(readFileSync(resolve(stylesRoot, rel), 'utf8'));
const ALL_RULES = styleSheetPaths(stylesRoot).flatMap((path: string) =>
  cssRules(readFileSync(path, 'utf8')).map(rule => ({ ...rule, path })));

const SPACER = '.content-pane-body > :not(.content-view-full-bleed)::after';

/** The classes a view root carries, as ContentPane mounts them. */
const VIEW_ROOT = /\.(content-view|panel-content|settings-panel|apps-view|plugins-view|notification-detail|inline-form|file-preview-frame|app-ui-inline|url-preview-inline)\b/;

/** A selector's last compound, the element it styles, with every functional
 *  pseudo-class argument dropped so a `:has(...)` cannot pose as the subject. */
function subject(selector: string): string {
  let flat = selector;
  while (/\([^()]*\)/.test(flat)) flat = flat.replace(/\([^()]*\)/g, '');
  return flat.trim().split(/[\s>+~]+/).pop() ?? '';
}

/** Each full-bleed view fills the pane and scrolls inside itself, with why. */
const FULL_BLEED: Record<string, string> = {
  'components/files/FilesView.tsx': 'the whole pane is the import drop zone',
  'components/apps/AppUiInline.tsx': 'the app iframe fills the pane',
  'components/files/UrlPreviewInline.tsx': 'the page iframe fills the pane',
  'components/layout/ContentPane.tsx': 'a file preview scrolls inside itself (a phone opts it back in)',
};

describe('the content pane end space', () => {
  it('is one default-deny spacer after every view', () => {
    const rule = sheet('panels/content.css').find(r => r.selector === SPACER);
    expect(rule, `no rule "${SPACER}"`).toBeDefined();
    expect(rule!.props.get('content')).toBe("''");
    expect(rule!.props.get('display')).toBe('block');
    expect(rule!.props.get('height')).toBe('var(--pane-end-space)');
  });

  it('clears the home indicator on a phone', () => {
    const pane = sheet('panels/shell.css').find(r => r.selector === '.content-pane-body');
    expect(pane?.props.get('--pane-end-space')).toContain('var(--safe-area-bottom)');
  });

  // A pinned height lets a long view overflow its own box, so the spacer
  // after the box sits under the content instead of below it.
  it('follows content: no sheet pins a column view to a fixed height', () => {
    const pinned = ALL_RULES.filter(rule =>
      rule.props.has('height') &&
      selectorList(rule.selector).some(s =>
        /(\.content-view\.active|\.panel-content|\.notification-detail)$/.test(s) &&
        !s.includes('full-bleed')));
    expect(pinned.map(r => `${relative(stylesRoot, r.path)}: ${r.selector}`)).toEqual([]);
  });

  // The phone's flowing file preview is the one sanctioned second rule. It
  // re-adds the same spacer to a full-bleed view, and nothing else may touch one.
  it('is styled in one place plus the phone opt-in, so no view can cancel it', () => {
    const touching = ALL_RULES.filter(rule => selectorList(rule.selector).some(s =>
      s.endsWith('::after') && (/\.content-pane-body\s*>/.test(s) || VIEW_ROOT.test(subject(s)))));
    const optIn = touching.filter(r => r.selector.includes('.file-preview-frame:has('));
    expect(optIn.length, 'the phone opt-in rule is gone').toBe(1);
    expect(optIn[0].atRules).toContain('@media (--phone-layout)');
    expect(optIn[0].props.get('height')).toBe('var(--pane-end-space)');
    const others = touching.filter(r => r.selector !== SPACER && r !== optIn[0]);
    expect(others.map(r => `${relative(stylesRoot, r.path)}: ${r.selector}`)).toEqual([]);
  });

  it('opts a view out only through a reviewed full-bleed mark', () => {
    const marked = clientSourcePaths(srcRoot)
      .filter((path: string) => readFileSync(path, 'utf8').includes('content-view-full-bleed'))
      .map((path: string) => relative(srcRoot, path))
      .sort();
    expect(marked, 'a new full-bleed view drops the end space; add it to FULL_BLEED with why')
      .toEqual(Object.keys(FULL_BLEED).sort());
  });
});
