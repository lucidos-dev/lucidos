/**
 * The header's pane dividers run to the window's top edge.
 *
 * The bug: on the packaged macOS build a theme with a visible
 * `--header-divider` drew both dividers from the header's top edge down. The
 * reclaimed title-bar band above stayed unbroken, so each divider read as a
 * stub hanging from the band. The focus wash already rises through the band
 * by the same `--titlebar-inset`, so the dividers now match it.
 *
 * Off the macOS Tauri build `--titlebar-inset` is unset, so the rise is 0px.
 *
 * A SOURCE SCAN because no WebDriver reaches WKWebView (ADR 0016).
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const shellCss: string = readFileSync(resolve(here, '..', 'panels/shell.css'), 'utf-8');

const RISE = 'calc(-1 * var(--titlebar-inset, 0px))';

describe('header dividers rise through the title-bar band', () => {
  for (const divider of ['.app-header::before', '.app-header::after']) {
    it(`${divider} starts at the window's top edge`, () => {
      const placed = cssRules(shellCss).filter(
        r => r.selector === divider && r.props.has('top'),
      );
      expect(placed.length, `expected one rule placing ${divider}`).toBe(1);
      expect(placed[0].props.get('top')).toBe(RISE);
    });
  }

  it('rises exactly as far as the focus wash', () => {
    const wash = cssRules(shellCss).find(
      r => r.selector === '.header-focus-wash' && r.props.has('top'),
    );
    expect(wash?.props.get('top')).toBe(RISE);
  });
});
