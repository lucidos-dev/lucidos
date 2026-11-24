import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';

import { rulesTargeting, selectorList, styleSheetPaths } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const STYLES_ROOT: string = resolve(here, '..');

const SHEETS: Array<{ file: string; css: string }> = styleSheetPaths(STYLES_ROOT).map(
  (path: string) => ({ file: relative(STYLES_ROOT, path), css: readFileSync(path, 'utf8') }),
);

const SURFACES = [
  'progress-dialog',
  'confirm-details',
  'dialog-body',
  'step-detail-body',
  'explainer-body',
  'event-condition-popover-body',
  'toast-heading',
  'toast-text',
] as const;

const STATES = [':focus', ':focus-visible'] as const;

const OUTLINE_PROPS = ['outline', 'outline-style'] as const;

interface OutlineDecl { state: string; prop: string; value: string; file: string }

function outlinesInFocus(surface: string): OutlineDecl[] {
  const found: OutlineDecl[] = [];
  for (const { file, css } of SHEETS) {
    for (const rule of rulesTargeting(css, surface)) {
      const members = selectorList(rule.selector);
      for (const state of STATES) {
        const hit = members.some(
          one => one.includes(`.${surface}${state}`) && !one.includes(`.${surface}${state}-`),
        );
        if (!hit) continue;
        for (const prop of OUTLINE_PROPS) {
          const value = rule.props.get(prop);
          if (value !== undefined) found.push({ state, prop, value, file });
        }
      }
    }
  }
  return found;
}

describe('non-control surfaces paint no focus ring', () => {
  for (const surface of SURFACES) {
    const found = outlinesInFocus(surface);

    it(`.${surface} suppresses the outline in both focus states`, () => {
      const missing = STATES.filter(
        state => !found.some(f => f.state === state && f.prop === 'outline' && f.value === 'none'),
      );
      expect(
        missing,
        `.${surface} takes focus without being a control, so it owes `
        + '`outline: none` for both :focus and :focus-visible. Without it the browser frames '
        + 'a block of text in its default ring, which is what the restart dialog was reported for.',
      ).toEqual([]);
    });

    it(`.${surface} has no rule putting the outline back`, () => {
      const revived = found.filter(f => f.value !== 'none')
        .map(f => `${f.file}: .${surface}${f.state} { ${f.prop}: ${f.value} }`);
      expect(
        revived,
        'A ring on one of these boxes reads as a control the user can press, and none of them is. '
        + 'Style the control inside it instead.',
      ).toEqual([]);
    });
  }
});

const TAB_EXCLUDED: ReadonlyArray<readonly [cls: string, source: string]> = [
  ['confirm-details', 'components/shared/ConfirmDialog.tsx'],
  ['toast-heading', 'components/shared/Toast.tsx'],
  ['toast-text', 'components/shared/Toast.tsx'],
];

const DECLARED_REGIONS: ReadonlyArray<readonly [cls: string, source: string]> = [
  ['file-preview-modal-body', 'components/files/FilePreviewModal.tsx'],
];

const SRC_ROOT: string = resolve(here, '..', '..');

function openingTag(cls: string, source: string): string {
  const src: string = readFileSync(resolve(SRC_ROOT, source), 'utf8');
  const tag = src.match(new RegExp(`<[a-z]+[^>]*\\sclass="${cls}"[^>]*>`));
  expect(tag, `no element renders class="${cls}" in ${source}`).not.toBeNull();
  return tag![0];
}

describe('a scroll box states its own Tab contract', () => {
  for (const [cls, source] of TAB_EXCLUDED) {
    it(`.${cls} renders with tabIndex={-1}`, () => {
      expect(
        openingTag(cls, source),
        `.${cls} overflows, and Chrome promotes an overflowing scroller with no focusable child `
        + 'to a Tab stop. Its surface traps Tab elsewhere and cannot name that stop, so declare '
        + '`tabIndex={-1}` and let the surface\'s own cycle stay the whole truth.',
      ).toContain('tabIndex={-1}');
    });
  }

  for (const [cls, source] of DECLARED_REGIONS) {
    it(`.${cls} renders as a named, reachable scroll region`, () => {
      const tag = openingTag(cls, source);
      expect(
        tag,
        `.${cls} is the only way a keyboard scrolls a long preview, so it stays a Tab stop in `
        + 'every browser rather than only where Chrome promotes it.',
      ).toContain('tabIndex={0}');
      expect(tag).toContain('role="region"');
      expect(tag).toMatch(/aria-label="[^"]+"/);
    });

    it(`.${cls} shows where focus went`, () => {
      const ring = outlinesInFocus(cls).filter(f => f.state === ':focus-visible');
      expect(
        ring.every(f => f.value !== 'none'),
        `.${cls} is a Tab stop the user reaches on purpose, so a sighted keyboard user has to `
        + 'see it. Suppress the ring only on a box nobody can Tab to.',
      ).toBe(true);
      const shadowed = SHEETS.some(({ css }) => rulesTargeting(css, cls).some(
        rule => selectorList(rule.selector).some(one => one.includes(`.${cls}:focus-visible`))
          && (rule.props.get('box-shadow') ?? '').includes('--focus-ring'),
      ));
      expect(shadowed, `.${cls}:focus-visible owes the shared --focus-ring band`).toBe(true);
    });
  }
});
