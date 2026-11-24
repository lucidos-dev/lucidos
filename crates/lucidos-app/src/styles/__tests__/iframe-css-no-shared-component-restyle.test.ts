import { describe, it, expect } from 'vitest';
import postcss from 'postcss';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { cssRules, rulesTargeting } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const STYLES_DIR = resolve(here, '..');
const REPO_ROOT = resolve(here, '../../../../..');

const SHARED_FILES = [
  resolve(STYLES_DIR, 'global/shared-components.css'),
  resolve(STYLES_DIR, 'global/surface.css'),
  resolve(STYLES_DIR, 'global/text-input.css'),
];

const IFRAME_ONLY_FILES = [
  resolve(REPO_ROOT, 'crates/lucidos-agents-api/src/api/sdk_iframe.css'),
  resolve(REPO_ROOT, 'crates/lucidos-agents-api/src/api/sdk_iframe_shapes.css'),
  resolve(REPO_ROOT, 'crates/lucidos-agents-api/src/api/sdk_iframe_control_theme_parts.css'),
];

const read = (path: string): string => readFileSync(path, 'utf8');

function baseClassNames(css: string): string[] {
  const names = new Set<string>();
  for (const rule of cssRules(css)) {
    for (const one of postcss.list.comma(rule.selector)) {
      const m = one.trim().match(/^\.([\w-]+)$/);
      if (m) names.add(m[1]);
    }
  }
  return [...names];
}

const BANNED_PROPS = [
  /^color$/,
  /^background/,
  /^border/,
  /^padding/,
  /^font-size$/,
  /^box-shadow$/,
];

function isBanned(prop: string): boolean {
  return BANNED_PROPS.some(re => re.test(prop));
}

const ALLOWED: ReadonlyArray<{ selector: string; prop: string }> = [
  { selector: '.list-row-info', prop: 'font-size' },
];

function isAllowed(selector: string, prop: string): boolean {
  return ALLOWED.some(a => a.selector === selector && a.prop === prop);
}

function isPartToken(value: string): boolean {
  return /var\(\s*--part-/.test(value);
}

describe('no iframe-only file restyles a shared component class', () => {
  const sharedClasses = SHARED_FILES.flatMap(path => baseClassNames(read(path)));

  it('found at least the dropdown and text-input classes', () => {
    for (const name of ['dropdown-trigger', 'dropdown-option', 'surface-box', 'text-input']) {
      expect(sharedClasses, name).toContain(name);
    }
  });

  for (const path of IFRAME_ONLY_FILES) {
    const fileLabel = path.split('/crates/')[1];
    const css = read(path);

    it.each(sharedClasses)(`.%s carries no visual property in ${fileLabel}`, className => {
      const offenders: string[] = [];
      for (const rule of rulesTargeting(css, className)) {
        for (const [prop, value] of rule.props) {
          if (isBanned(prop) && !isAllowed(rule.selector, prop) && !isPartToken(value)) {
            offenders.push(`${rule.selector} { ${prop}: ${value} }`);
          }
        }
      }
      expect(
        offenders,
        `${fileLabel} restyles a shared component class (.${className}). `
        + 'Edit the shared rule in shared-components.css / surface.css / '
        + 'text-input.css instead — see docs/plans/2026-10-03-sdk-dropdown-shares-host-css.md.',
      ).toEqual([]);
    });
  }
});
