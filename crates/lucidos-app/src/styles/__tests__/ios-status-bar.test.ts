/**
 * The installed iOS app runs under a translucent status bar, so the header
 * fills the Dynamic Island area. Its controls sit below iOS 27's top-edge blur,
 * by a clearance only that app sets. iOS 27 ignores an unpainted strip, so no
 * strip exists (ADR 0553).
 *
 * `theme-color` carries the header's top colour for the other platforms'
 * toolbars. Every script reads that colour from `THEME_MODE_HEADER_TOP`. The
 * stylesheet and index.html cannot import it, so this pins them to it.
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
const mobileCss: string = readFileSync(resolve(stylesDir, 'mobile.css'), 'utf-8');
const drawerCss: string = readFileSync(resolve(stylesDir, 'drawer.css'), 'utf-8');

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
  it('is translucent, so the header runs up behind it', () => {
    const tags = indexHtml.match(/<meta name="apple-mobile-web-app-status-bar-style"[^>]*>/g) ?? [];
    expect(tags.length, 'index.html must declare the style exactly once').toBe(1);
    expect(tags[0]).toContain('content="black-translucent"');
  });

  it('carries no strip and no launch mode in the first document', () => {
    expect(indexHtml).not.toContain('status-bar-fill');
    expect(indexHtml).not.toContain('data-ios-status-bar');
    expect(indexHtml).not.toContain('lucidos-status-bar-mode');
  });

  it('pads the mobile header controls by the inset plus a clearance that is 0 unless set', () => {
    expect(rootToken(':root', '--status-bar-blur-clearance')).toBe('0px');
    expect(rootToken(':root', '--header-controls-top')).toBe(
      'calc(var(--safe-area-top) + var(--status-bar-blur-clearance))',
    );
    const header = cssRules(mobileCss).find((r) => r.selector === '.app-header' && r.props.has('padding-top'));
    expect(header?.props.get('padding-top')).toBe('var(--header-controls-top)');
  });

  it('measures the header height from the same controls top before the header is measured', () => {
    for (const css of [mobileCss, drawerCss]) {
      expect(css).toContain('var(--mobile-header-height, calc(2.75rem + var(--header-controls-top)))');
      expect(css).not.toContain('var(--mobile-header-height, calc(2.75rem + var(--safe-area-top)))');
    }
  });

  it('takes the header top colour the stylesheet paints, per mode', () => {
    expect(THEME_MODE_HEADER_TOP).toEqual(headerTop());
  });

  it('starts on the dark header top colour before any script runs', () => {
    expect(indexHtml).toContain(`<meta name="theme-color" content="${THEME_MODE_HEADER_TOP.dark}">`);
  });
});
