/**
 * The host components apps get are the host's own, from one stylesheet.
 *
 * The switch, the spinner, the label tones and the pill bar each live in
 * `styles/global/shared-components.css`. The host imports that file and the
 * engine appends it to the served `/api/v1/sdk-iframe.css`, so an app draws
 * exactly what the host draws.
 *
 * Two things break that silently. A second base rule anywhere else reaches the
 * host alone, and the two copies drift. A token only `base.css` defines paints
 * nothing in an app frame, which never loads it.
 *
 * A host rule that restyles one in context (`.thread-status .mini-spinner`) is
 * fine: it is not a second definition, and no app matches it.
 */
import { describe, it, expect } from 'vitest';
import postcss, { type Root } from 'postcss';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

import { cssRules, rulesTargeting, styleSheetPaths } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT: string = resolve(here, '../../../../..');
const STYLES_DIR: string = resolve(here, '..');
const SHARED_CSS: string = resolve(STYLES_DIR, 'global/shared-components.css');
const HOST_TOKENS_CSS: string = resolve(STYLES_DIR, 'global/base.css');
const IFRAME_CSS: string = resolve(REPO_ROOT, 'crates/lucidos-engine/src/api/sdk_iframe.css');

const EXPORTED: string[] = [
  'toggle-switch',
  'toggle-slider',
  'mini-spinner',
  'pill-bar',
  'pill-bar-btn',
  'label-success',
  'label-warning',
  'label-error',
  'label-neutral',
  'dropdown-trigger',
  'dropdown-chevron',
  'dropdown-placeholder',
  'dropdown-option',
];

const read = (path: string): string => readFileSync(path, 'utf-8');
const parse = (path: string): Root => postcss.parse(read(path), { from: path });

/** Rules that define the class itself: `.x` alone in a selector-list member,
 *  at the TOP LEVEL. A media-nested one is a refinement, gated by a
 *  condition. mobile.css bumps `.dropdown-chevron`'s size under
 *  `pointer: coarse`. That is the same "restyles in context" shape a
 *  descendant selector already gets a pass for above, not a rival
 *  definition. */
function baseRules(css: string, className: string): string[] {
  return cssRules(css)
    .filter(rule => rule.atRules === ''
      && postcss.list.comma(rule.selector).some(one => one.trim() === `.${className}`))
    .map(rule => rule.selector);
}

function keyframes(root: Root, name: string): number {
  let count = 0;
  root.walkAtRules('keyframes', at => {
    if (at.params === name) count++;
  });
  return count;
}

function declaredTokens(root: Root): Set<string> {
  const names = new Set<string>();
  root.walkDecls(decl => {
    if (decl.prop.startsWith('--')) names.add(decl.prop);
  });
  return names;
}

const otherSheets = (): string[] =>
  [...styleSheetPaths(resolve(STYLES_DIR, '..')), IFRAME_CSS].filter(p => p !== SHARED_CSS);

describe('the host components apps get come from one stylesheet', () => {
  it.each(EXPORTED)('defines .%s in shared-components.css', className => {
    expect(baseRules(read(SHARED_CSS), className)).not.toEqual([]);
  });

  it('defines none of them a second time anywhere else', () => {
    const copies: string[] = [];
    for (const path of otherSheets()) {
      const css = read(path);
      for (const className of EXPORTED) {
        for (const selector of baseRules(css, className)) {
          copies.push(`${path.split('/crates/')[1]}: ${selector}`);
        }
      }
    }
    expect(
      copies,
      'A base rule outside shared-components.css reaches the host alone, so apps '
      + 'draw a different component. Edit the shared rule instead.',
    ).toEqual([]);
  });

  it('declares the spinner keyframes once, beside the spinner', () => {
    expect(keyframes(parse(SHARED_CSS), 'spin')).toBe(1);
    for (const path of otherSheets()) {
      expect(keyframes(parse(path), 'spin'), `${path} declares @keyframes spin again`).toBe(0);
    }
  });

  it('reads only tokens that resolve in the host AND in an app frame', () => {
    const host = declaredTokens(parse(HOST_TOKENS_CSS));
    const iframe = declaredTokens(parse(IFRAME_CSS));
    const shared = read(SHARED_CSS);
    const ownTokens = declaredTokens(parse(SHARED_CSS));
    const unresolved: string[] = [];
    for (const className of EXPORTED) {
      for (const rule of rulesTargeting(shared, className)) {
        for (const value of rule.props.values()) {
          for (const [, name, comma] of value.matchAll(/var\(\s*(--[\w-]+)\s*(,)?/g)) {
            if (comma || ownTokens.has(name)) continue;
            if (!host.has(name)) unresolved.push(`${name} (base.css) in ${rule.selector}`);
            if (!iframe.has(name)) unresolved.push(`${name} (sdk_iframe.css) in ${rule.selector}`);
          }
        }
      }
    }
    expect(
      unresolved,
      "An app frame never loads base.css. Mirror the token into the engine's "
      + 'sdk_iframe.css, or give the read a var() fallback.',
    ).toEqual([]);
  });
});
