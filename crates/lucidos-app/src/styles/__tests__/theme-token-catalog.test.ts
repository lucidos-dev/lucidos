/**
 * The theme token catalog lists every token a theme can tune, and says what each
 * one is by default.
 *
 * The catalog (`core/themes/theme-tokens.json` in the engine) is what a
 * theme-building plugin reads from `GET /api/v1/themes/tokens`. It is only
 * honest while it matches the stylesheet, so this pins the two together:
 *
 *   - every custom property in the dark and light blocks is in the catalog;
 *   - every catalog default is the value `base.css` gives in that mode;
 *   - a token the catalog says reaches app frames is defined for them.
 *
 * The header block has its own guard: its base tokens live on `<html>`, where a
 * theme's inline value reaches them, never on `.pane-header`.
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
const repo: string = resolve(here, '../../../../..');
const read = (path: string): string => readFileSync(resolve(repo, path), 'utf-8');

const baseCss = read('crates/lucidos-app/src/styles/global/base.css');
const shellCss = read('crates/lucidos-app/src/styles/panels/shell.css');
const iframeCss = read('crates/lucidos-engine/src/api/sdk_iframe.css');

interface CatalogToken {
  name: string;
  group: string;
  kind: string;
  label: string;
  description: string;
  frames: boolean;
  default: { dark: string; light: string };
}
interface Catalog {
  seeds: string[];
  groups: { id: string }[];
  tokens: CatalogToken[];
}
const catalog: Catalog = JSON.parse(read('crates/lucidos-engine/src/core/themes/theme-tokens.json'));

/** Custom properties a selector's top-level rule declares, last one winning. */
function tokensOf(css: string, selector: string): Map<string, string> {
  const rules = cssRules(css).filter(r => r.selector === selector && r.atRules === '');
  expect(rules.length, `no top-level ${selector} rule`).toBeGreaterThan(0);
  const out = new Map<string, string>();
  for (const rule of rules) {
    for (const [prop, value] of rule.props) if (prop.startsWith('--')) out.set(prop, value);
  }
  return out;
}

const root = tokensOf(baseCss, ':root');
// The dark block's selector is `html, html[data-theme-mode="dark"]`, so it applies
// under the light theme too. Light therefore falls back to it before `:root`.
const dark = tokensOf(baseCss, 'html, html[data-theme-mode="dark"]');
const light = tokensOf(baseCss, 'html[data-theme-mode="light"]');

function stylesheetDefault(name: string): { dark?: string; light?: string } {
  return {
    dark: dark.get(name) ?? root.get(name),
    light: light.get(name) ?? dark.get(name) ?? root.get(name),
  };
}

describe('the theme token catalog', () => {
  const names = catalog.tokens.map(t => t.name);

  it('names each token once', () => {
    expect(new Set(names).size).toBe(names.length);
  });

  it('lists every token the theme blocks declare', () => {
    // The protected palette is the engine's to derive, never a theme's to set
    // (ADR 0309), so it stays out of the catalog.
    const declared = new Set(
      [...dark.keys(), ...light.keys()].filter(name => !name.startsWith('--protected-')),
    );
    const missing = [...declared].filter(name => !names.includes(name));
    expect(missing, 'add these to theme-tokens.json').toEqual([]);
  });

  it('gives each token the default base.css gives it, per mode', () => {
    for (const token of catalog.tokens) {
      const css = stylesheetDefault(token.name);
      expect(css.dark, `${token.name} is not declared in base.css`).toBeDefined();
      expect(token.default, token.name).toEqual(css);
    }
  });

  it('puts every token in a known group, with a label and a description', () => {
    const groups = catalog.groups.map(g => g.id);
    for (const token of catalog.tokens) {
      expect(groups, token.name).toContain(token.group);
      expect(token.label.length, token.name).toBeGreaterThan(0);
      expect(token.description.length, token.name).toBeGreaterThan(0);
    }
  });

  it('marks a token as reaching app frames exactly when sdk_iframe.css defines it', () => {
    const inFrames = (name: string) => new RegExp(`^\\s*${name}\\s*:`, 'm').test(iframeCss);
    for (const token of catalog.tokens) {
      expect(token.frames, token.name).toBe(inFrames(token.name));
    }
  });

  it('keeps the three seeds in the catalog', () => {
    for (const seed of catalog.seeds) expect(names).toContain(seed);
  });
});

describe('the header follows the theme', () => {
  const headerTokens = catalog.tokens.filter(t => t.group === 'header').map(t => t.name);

  it('declares no header token on .pane-header, where it would beat the theme', () => {
    const paneHeader = cssRules(shellCss).filter(r => r.selector === '.pane-header');
    for (const rule of paneHeader) {
      for (const name of headerTokens) {
        expect(rule.props.has(name), `${name} is redeclared on .pane-header`).toBe(false);
      }
    }
  });

  it('paints no header rule in a literal white, which a theme could not retune', () => {
    const mobileCss = read('crates/lucidos-app/src/styles/mobile.css');
    const white = /#fff\b|#ffffff\b|rgba\(\s*255\s*,\s*255\s*,\s*255/i;
    for (const rule of [...cssRules(shellCss), ...cssRules(mobileCss)]) {
      if (!/header/.test(rule.selector)) continue;
      expect(rule.body, `${rule.selector} hardcodes white`).not.toMatch(white);
    }
  });

  it('builds the muted foreground from --header-fg, not from a literal white', () => {
    expect(dark.get('--header-fg-muted')).toContain('var(--header-fg)');
  });

  it('draws the focus underline from its own token, transparent by default', () => {
    expect(dark.get('--focus-header-underline')).toBe('transparent');
    expect(shellCss).toContain('var(--focus-header-underline)');
  });
});
