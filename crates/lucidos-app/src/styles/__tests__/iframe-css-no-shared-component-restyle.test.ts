/**
 * Structural guard against the exact drift this repo once shipped: a shared
 * component class (the host's dropdown, now also `lucidos.ui.Select`'s)
 * quietly grows an iframe-only visual override, and the two controls drift
 * apart again. See docs/plans/2026-10-03-sdk-dropdown-shares-host-css.md.
 *
 * `shared-components.css`, `surface.css` and `text-input.css` are the engine's
 * reusable layer (`crates/lucidos-engine/src/api/sdk.rs`). This test derives
 * every class they define as a base rule. Two iframe-only files are scanned:
 * `sdk_iframe.css` and `sdk_iframe_control_theme_parts.css`. Neither may set a
 * VISUAL property (colour, background, border, radius, padding, font-size,
 * box-shadow) on a selector naming one of them. It does not compare values: a
 * class nobody has written an override for yet is still covered, because the
 * ban is on the property, not a stale list.
 *
 * One exemption: a declaration whose value reads a `--part-*` token. That is
 * the theme-parts mechanism itself (ADR 0307), a catalog-governed
 * customization channel, not a restyle — `sdk_iframe_control_theme_parts.css`
 * legitimately sets `.text-input`'s `border-color`/`box-shadow` this way.
 */
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
  resolve(REPO_ROOT, 'crates/lucidos-engine/src/api/sdk_iframe.css'),
  resolve(REPO_ROOT, 'crates/lucidos-engine/src/api/sdk_iframe_control_theme_parts.css'),
];

const read = (path: string): string => readFileSync(path, 'utf8');

/** Every class named alone in a selector-list member — a base rule, not a
 *  state/variant compound like `.dropdown-option.active`. Mirrors the
 *  `baseRules` helper in `exported-components-single-source.test.ts`. */
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

/** Visual properties a shared component's look must come from ONE place.
 *  Layout (display, position, width, z-index, flex*, gap, …) is fine to
 *  restate in an iframe-only rule — only these are banned. */
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

/** Documented, pre-existing exceptions: a compensation for how the iframe's
 *  OWN tokens differ from the host's, not a restyle to look different. Each
 *  needs the comment at its site to justify it; add here only with one. */
const ALLOWED: ReadonlyArray<{ selector: string; prop: string }> = [
  // The iframe's `body` is the chat prose step (`--font-size-sm`, ADR 0319),
  // not the host's `md`. `.list-row-info` restates `md` so a row inside an
  // app reads at the same size as the identical row in the host shell.
  { selector: '.list-row-info', prop: 'font-size' },
];

function isAllowed(selector: string, prop: string): boolean {
  return ALLOWED.some(a => a.selector === selector && a.prop === prop);
}

/** A `--part-*` read is the theme-parts mechanism's own customization
 *  channel (ADR 0307), not a restyle — see the exemption note above. */
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
