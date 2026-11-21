/**
 * No realm builds CSS text from theme data (ADR 0307).
 *
 * A theme's tokens and part tokens reach a page only through `setProperty` on
 * one custom property at a time (`replaceInlineTokens`). A value there cannot
 * break out of its declaration. A stylesheet built from the same data could,
 * so this pins that no shipping source has a way to build one:
 *
 *   - nothing creates a `<style>` element or a constructed stylesheet, or
 *     calls `insertRule`, anywhere in the shell or the SDK;
 *   - the one `<style>` string in the shell is the file preview's zoom, which
 *     carries a number and no theme data;
 *   - the modules that handle theme data write no `cssText` or `textContent`.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, relative, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { clientSourcePaths } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const app: string = resolve(here, '../..');
const repo: string = resolve(here, '../../../../..');
const sdk: string = resolve(repo, 'packages/lucidos-sdk/src');

const sources = [...clientSourcePaths(app), ...clientSourcePaths(sdk)]
  .map(path => ({ file: relative(repo, path), text: readFileSync(path, 'utf-8') as string }));

/** The modules a theme's tokens pass through on their way to the page. */
const THEME_DATA_MODULES = [
  'packages/lucidos-sdk/src/appearance.ts',
  'packages/lucidos-sdk/src/themeParts.ts',
  'packages/lucidos-sdk/src/ui.ts',
  'packages/lucidos-sdk/src/boot/appearanceBoot.ts',
  'crates/lucidos-app/src/store/actions/preferences.ts',
  'crates/lucidos-app/src/utils/themeEffects.ts',
];

describe('theme data never becomes CSS text', () => {
  it('scans the shell and the SDK', () => {
    expect(sources.length).toBeGreaterThan(100);
    for (const file of THEME_DATA_MODULES) expect(sources.map(s => s.file)).toContain(file);
  });

  it('creates no stylesheet anywhere', () => {
    const sink = /createElement\(\s*['"]style['"]|insertRule\s*\(|new\s+CSSStyleSheet|adoptedStyleSheets/;
    expect(sources.filter(s => sink.test(s.text)).map(s => s.file)).toEqual([]);
  });

  it('writes a <style> string only for the file preview zoom', () => {
    expect(sources.filter(s => /<style/.test(s.text)).map(s => s.file))
      .toEqual(['crates/lucidos-app/src/components/files/previewIframeLinks.ts']);
  });

  it('sets no cssText or textContent in the modules that carry theme data', () => {
    for (const { file, text } of sources.filter(s => THEME_DATA_MODULES.includes(s.file))) {
      expect(text, file).not.toMatch(/\.cssText\s*=|\.textContent\s*=/);
    }
  });
});
