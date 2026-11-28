import { describe, it, expect, beforeAll } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: a plain .mjs build helper, shared with `npm run build`
import { BOOT_BUNDLES, buildBootBundle } from '../../appearanceBoot.build.mjs';
import { PREF_FONT_FAMILY, PREF_THEME_MODE, PREF_UI_SCALE } from '../generated/preference-catalog';

const here = dirname(fileURLToPath(import.meta.url));
const PKG_ROOT = resolve(here, '../..');

let script = '';

beforeAll(async () => {
  const iframe = (BOOT_BUNDLES as Array<{ entry: string; out: string }>)
    .find((b) => b.out.endsWith('appearance-boot.iframe.js'));
  if (!iframe) throw new Error('BOOT_BUNDLES has no iframe bundle');
  script = await buildBootBundle(resolve(PKG_ROOT, iframe.entry));
});

describe('the iframe appearance boot bundle', () => {
  it('is the built bundle, wrapped so its locals never reach the global scope', () => {
    expect(script).toContain('GENERATED from packages/lucidos-sdk/src/boot/');
    expect(script).toContain('(() => {');
    expect(script.trimEnd().endsWith('})();')).toBe(true);
  });

  it('is the iframe build, not the shell one', () => {
    expect(script, 'the boot splash belongs to the shell').not.toContain('radial-gradient');
    expect(script, 'theme telemetry belongs to the shell').not.toContain('client-log');
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
    expect(script).toContain('served[PREF_UI_SCALE.key]');
    expect(script).toContain('served["text-size"]');
    expect(script).toContain('wsLocalGet("lucidos-ui-scale")');
  });

  it('namespaces keys per workspace, direct and behind a gateway', () => {
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
    expect(script).toContain('d.style.background =');
  });

  it('never sets font-feature-settings directly', () => {
    expect(script).not.toContain('setProperty("font-feature-settings"');
  });

  it('still applies the style remote', () => {
    expect(script).toContain('lucidos-style-overrides');
  });
});
