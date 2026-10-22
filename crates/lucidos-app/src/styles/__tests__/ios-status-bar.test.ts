/**
 * The installed iOS app runs below an opaque status bar. Under black-translucent,
 * iOS 26+ blurs the top of the page, the header included, and no page paint can
 * stop it (ADR 0548). The opaque bar takes `theme-color`, which therefore
 * carries the header's top colour.
 *
 * Every script reads that colour from `THEME_MODE_HEADER_TOP`. The stylesheet
 * and index.html's static tag cannot import it, so this pins them to it.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { THEME_MODE_HEADER_TOP } from '@lucidos/appearance';

import { cssRules } from './css-rule-helpers';

const stylesDir: string = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const indexHtml: string = readFileSync(resolve(stylesDir, '../../index.html'), 'utf-8');
const baseCss: string = readFileSync(resolve(stylesDir, 'global/base.css'), 'utf-8');

function rootToken(selector: string, token: string): string {
  const value = cssRules(baseCss).find((r) => r.selector === selector && r.props.has(token))?.props.get(token);
  expect(value, `${selector} must set ${token}`).toBeTruthy();
  return value as string;
}

/** The header's top colour per mode, `var()` resolved against `:root`. */
function headerTop(): { light: string; dark: string } {
  const resolveVar = (value: string): string => {
    const name = /^var\((--[\w-]+)\)$/.exec(value)?.[1];
    return name ? rootToken(':root', name) : value;
  };
  return {
    light: resolveVar(rootToken('html[data-theme-mode="light"]', '--header-bar-top')),
    dark: resolveVar(rootToken('html, html[data-theme-mode="dark"]', '--header-bar-top')),
  };
}

describe('the iOS status bar', () => {
  it('is opaque, never black-translucent', () => {
    const tags = indexHtml.match(/<meta name="apple-mobile-web-app-status-bar-style"[^>]*>/g) ?? [];
    expect(tags.length, 'index.html must declare the style exactly once').toBe(1);
    expect(tags[0]).not.toContain('black-translucent');
  });

  it('takes the header top colour the stylesheet paints, per mode', () => {
    expect(THEME_MODE_HEADER_TOP).toEqual(headerTop());
  });

  it('starts on the dark header top colour before any script runs', () => {
    expect(indexHtml).toContain(`<meta name="theme-color" content="${THEME_MODE_HEADER_TOP.dark}">`);
  });
});
