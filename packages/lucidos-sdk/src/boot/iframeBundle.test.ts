/**
 * The bundle the engine serves to every app iframe as `/api/v1/sdk-prefs.js`,
 * built in memory through the module `npm run build` calls.
 *
 * The VALUES it applies are asserted once, at their source, in
 * `appearance.test.ts`, and its behaviour is driven against a fake document in
 * `appearanceBoot.test.ts`. What is left is the served artifact's side: it is
 * the boot script, it is complete, it is the IFRAME build, and it reads the
 * workspace-scoped keys the shell writes.
 */
import { describe, it, expect, beforeAll } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: a plain .mjs build helper, shared with `npm run build`
import { BOOT_BUNDLES, buildBootBundle } from '../../appearanceBoot.build.mjs';
import { PREF_FONT_FAMILY, PREF_THEME_MODE, PREF_UI_SCALE } from '../generated/preference-catalog';

const here = dirname(fileURLToPath(import.meta.url));
/** The SDK package root, from `packages/lucidos-sdk/src/boot/`. */
const PKG_ROOT = resolve(here, '../..');

let script = '';

beforeAll(async () => {
  const iframe = (BOOT_BUNDLES as Array<{ entry: string; out: string }>)
    .find((b) => b.out.endsWith('appearance-boot.iframe.js'));
  if (!iframe) throw new Error('BOOT_BUNDLES has no iframe bundle');
  // esbuild resolves the entry relative to cwd, which is the app crate when
  // Vitest runs, so build from an absolute path.
  script = await buildBootBundle(resolve(PKG_ROOT, iframe.entry));
});

describe('the iframe appearance boot bundle', () => {
  it('is the built bundle, wrapped so its locals never reach the global scope', () => {
    expect(script).toContain('GENERATED from packages/lucidos-sdk/src/boot/');
    // esbuild's IIFE wrapper is an arrow form, hence the loose check.
    expect(script).toContain('(() => {');
    expect(script.trimEnd().endsWith('})();')).toBe(true);
  });

  it('is the iframe build, not the shell one', () => {
    // The two entry points differ in exactly the pieces that are the SHELL's:
    // the boot-splash gradient and the theme-telemetry POST.
    expect(script, 'the boot splash belongs to the shell').not.toContain('radial-gradient');
    expect(script, 'theme telemetry belongs to the shell').not.toContain('client-log');
    // The shell clears the key before an iframe loads, so `?style-reset` must
    // be off here.
    expect(script).toContain('styleReset: false');
  });

  it('reads theme mode, font and scale from the seed, then storage', () => {
    for (const key of [PREF_THEME_MODE.key, PREF_FONT_FAMILY.key, PREF_UI_SCALE.key]) {
      expect(script, `must carry the generated catalog's ${key}`).toContain(`key: "${key}"`);
    }
    expect(script).toContain('THEME_MODE_KEY = PREF_THEME_MODE.key');
    expect(script).toContain('THEME_MODE_STORAGE_KEY = "lucidos-theme-mode"');
    expect(script).toContain('seeded(served, THEME_MODE_KEY, THEME_MODE_STORAGE_KEY)');
    expect(script).toContain('seeded(served, PREF_FONT_FAMILY.key, "lucidos-font-family")');
    // Scale reads the seed inline, because it carries the two pre-grid aliases.
    expect(script).toContain('served[PREF_UI_SCALE.key]');
    expect(script).toContain('served["text-size"]');
    expect(script).toContain('wsLocalGet("lucidos-ui-scale")');
  });

  it('namespaces keys per workspace, direct and behind a gateway', () => {
    // The iframe realm cannot reach the parent's Storage.prototype override, so
    // it derives the slug itself (ADR 0238).
    expect(script).toContain('indexOf("/app/")');
    expect(script).toContain('~cap');
    expect(script).toContain('ws:');
  });

  it('reads no per-workspace appearance key unscoped', () => {
    for (const key of [
      'lucidos-theme-mode',
      'lucidos-font-family',
      'lucidos-ui-scale',
      'lucidos-style-overrides',
    ]) {
      expect(script, `wrap ${key} in wsKey()`).not.toContain(`localStorage.getItem("${key}")`);
    }
  });

  it('resolves the system theme mode through matchMedia', () => {
    expect(script).toContain('matchMedia("(prefers-color-scheme: light)")');
  });

  it('sets data-theme-mode, the background and the font properties', () => {
    expect(script).toContain('THEME_MODE_ATTRIBUTE = "data-theme-mode"');
    expect(script).toContain('setAttribute(THEME_MODE_ATTRIBUTE, resolved)');
    expect(script).toContain('setProperty("--bg-primary"');
    expect(script).toContain('setProperty("--font-ui"');
    expect(script).toContain('setProperty("--font-features-text"');
    expect(script).toContain('setProperty("--font-features-code"');
  });

  it('paints the html background inline, for the iOS PWA first frame', () => {
    // Until the iframe's stylesheet applies, WKWebView's white shows through.
    expect(script).toContain('d.style.background =');
  });

  it('never sets font-feature-settings directly', () => {
    // The bare property is inherited, so it would ligature an app's prose too.
    expect(script).not.toContain('setProperty("font-feature-settings"');
  });

  it('still applies the style remote', () => {
    expect(script).toContain('lucidos-style-overrides');
  });
});
