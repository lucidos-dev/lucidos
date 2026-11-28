import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { FOLLOW_THEME, FONT_CATALOG } from '@lucidos/appearance';
import { PREF_FONT_FAMILY } from '@lucidos/preference-catalog';
import { fontOptions } from '../fontOptions';
import { SETTINGS_SUBVIEW_ITEMS } from '../../../store/store';
import { findSettingsEntry, settingsSearchEntryIds } from '../../search/searchIndex';
import { renderedSearchAnchors } from '../../search/__tests__/renderedSearchAnchors';

const here = dirname(fileURLToPath(import.meta.url));
const SETTINGS_VIEW = readFileSync(resolve(here, '..', 'SettingsView.tsx'), 'utf8');
const SYSTEM_SUBMENU = readFileSync(resolve(here, '..', 'SystemSubmenu.tsx'), 'utf8');
const NAV_ROW = readFileSync(resolve(here, '..', 'SettingsNavRow.tsx'), 'utf8');
const SETTINGS_HOME = readFileSync(resolve(here, '..', 'SettingsHome.tsx'), 'utf8');

function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^\\:])\/\/.*$/gm, '$1');
}

function functionBody(src: string, declaration: string): string {
  const stripped = stripComments(src);
  const start = stripped.indexOf(declaration);
  expect(start, `SettingsView.tsx must declare \`${declaration}\``).toBeGreaterThan(-1);
  const open = stripped.indexOf('{', start);
  let depth = 0;
  for (let i = open; i < stripped.length; i++) {
    if (stripped[i] === '{') depth++;
    else if (stripped[i] === '}' && --depth === 0) return stripped.slice(open + 1, i);
  }
  throw new Error(`unbalanced braces in \`${declaration}\``);
}

function expectRowGatedOnNothing(src: string, mapCall: string): void {
  const map = src.indexOf(mapCall);
  const row = src.indexOf('<SettingsNavRow', map);
  expect(map, `\`${mapCall}\``).toBeGreaterThan(-1);
  expect(row, 'the nav row element').toBeGreaterThan(map);
  expect(src.slice(map, row)).not.toMatch(/key === '[a-z-]+'/);
  expect(src.slice(map, row)).not.toMatch(/key !== '[a-z-]+'/);
}

describe('Settings nav structure', () => {
  it('renders every nav item on every platform: no category is platform-gated', () => {
    const home = stripComments(SETTINGS_HOME);
    expect(home).toMatch(/SETTINGS_NAV_ITEMS\.map\(/);
    expect(home).not.toMatch(/SETTINGS_NAV_ITEMS\.filter\(/);
    expectRowGatedOnNothing(home, 'SETTINGS_NAV_ITEMS.map(');
  });

  it('lists every System sub-page in the submenu, gated on nothing', () => {
    const submenu = stripComments(SYSTEM_SUBMENU);
    expect(submenu).toMatch(/SETTINGS_SYSTEM_SUBPANEL_ITEMS\.map\(/);
    expect(submenu).not.toMatch(/SETTINGS_SYSTEM_SUBPANEL_ITEMS\.filter\(/);
    expectRowGatedOnNothing(submenu, 'SETTINGS_SYSTEM_SUBPANEL_ITEMS.map(');
  });

  it('opens every drilldown row with a real button, in the one row both lists share', () => {
    const row = stripComments(NAV_ROW);
    expect(row).toContain('<button');
    expect(row).toContain('type="button"');
    expect(row).toContain('class="settings-section-title settings-nav-row"');
  });

  it('has a renderSubview case for every nav key, so no row opens onto nothing', () => {
    const body = functionBody(SETTINGS_VIEW, 'function renderSubview()');
    for (const { key } of SETTINGS_SUBVIEW_ITEMS) {
      expect(body, `renderSubview has no case for '${key}'`).toContain(`case '${key}':`);
    }
  });

  it('keeps both link settings in one section inside Appearance & Behavior, gated per ROW', () => {
    const links = functionBody(SETTINGS_VIEW, 'function linksSection()');
    expect(links).toMatch(/showExternalTarget\s*=\s*externalLinkTargetConfigurable\(\)/);
    expect(links).toMatch(/showInAppBrowser\s*=\s*isTauri\(\)/);
    expect(links).toMatch(/\{showExternalTarget && \(/);
    expect(links).toMatch(/\{showInAppBrowser && \(/);
    expect(functionBody(SETTINGS_VIEW, 'function appearanceSection()')).toContain('linksSection()');
    expect(functionBody(SETTINGS_VIEW, 'function renderSubview()'))
      .toContain(`case 'appearance': return appearanceSection();`);
  });

  it('lets the Links section vanish, but never the page around it', () => {
    const iface = functionBody(SETTINGS_VIEW, 'function appearanceSection()');
    expect(functionBody(SETTINGS_VIEW, 'function linksSection()')).toContain('return null');
    for (const anchor of ['appearance:theme', 'appearance:typography']) {
      const guarded = new RegExp(`\\S\\s*&&\\s*\\(\\s*<div class="settings-section">\\s*<div class="settings-section-title" data-search-anchor="${anchor}"`);
      expect(iface).toContain(`data-search-anchor="${anchor}"`);
      expect(iface, `${anchor} must render unconditionally`).not.toMatch(guarded);
    }
  });
});

describe('the Motion section', () => {
  it('follows the Theme section, on every client, and writes the preference', () => {
    const iface = functionBody(SETTINGS_VIEW, 'function appearanceSection()');
    expect(iface).toMatch(/<div class="settings-section-title" data-search-anchor="appearance:motion">Motion<\/div>/);
    expect(iface).not.toMatch(/&&\s*\(\s*<div class="settings-section">\s*<div class="settings-section-title" data-search-anchor="appearance:motion"/);
    const motionAt = iface.indexOf('appearance:motion');
    expect(motionAt).toBeGreaterThan(iface.indexOf('appearance:theme-effects'));
    expect(motionAt).toBeLessThan(iface.indexOf('appearance:typography'));
    expect(iface).toContain('{MOTION_PREFS.map((m) => (');
    expect(iface).toContain('onClick={() => void setMotion(m)}');
    expect(iface).toContain('const motion = motionPreference.value;');
  });
});

describe('the Effects row', () => {
  it('sits in the Theme section under the picker, on every client, and writes the preference', () => {
    const iface = functionBody(SETTINGS_VIEW, 'function appearanceSection()');
    expect(iface).toMatch(/<div class="settings-row" data-search-anchor="appearance:theme-effects">/);
    expect(iface).not.toMatch(/&&\s*\(\s*<div class="settings-row" data-search-anchor="appearance:theme-effects"/);
    const at = iface.indexOf('appearance:theme-effects');
    const pickerAt = iface.indexOf('<ThemePicker />');
    expect(pickerAt).toBeGreaterThan(iface.indexOf('appearance:mode'));
    expect(at).toBeGreaterThan(pickerAt);
    expect(at).toBeLessThan(iface.indexOf('appearance:motion'));
    expect(iface).toContain('{THEME_EFFECTS_PREFS.map((e) => (');
    expect(iface).toContain('onClick={() => void setThemeEffects(e)}');
    expect(iface).toContain('const themeEffects = themeEffectsPreference.value;');
  });
});

describe('the Font dropdown', () => {
  const options = fontOptions([]);

  it('lists the default first, as Mode and Motion list theirs', () => {
    expect(options[0]).toEqual({ value: FOLLOW_THEME, label: 'Follow the theme' });
    expect(PREF_FONT_FAMILY.fallback).toBe(FOLLOW_THEME);
    expect(SETTINGS_VIEW).toContain('? fontOptions(workspaceFontList.value.data.fonts, font)');
  });

  it('builds every catalog option from the font catalog', () => {
    const values = options.filter(o => !o.disabled).map(o => o.value).slice(1);
    expect(values).toEqual(FONT_CATALOG.filter(f => f.kind !== 'mono').map(f => f.id));
  });

  it('groups the fonts under Sans, Serif and Mono headers, in that order', () => {
    const headers = options.filter(o => o.disabled);
    expect(headers.map(o => o.label)).toEqual(['Sans', 'Serif', 'Mono']);
    for (const group of ['sans', 'serif', 'mono'] as const) {
      expect(FONT_CATALOG.some(f => f.group === group && f.kind !== 'mono')).toBe(true);
    }
  });
});

describe('Settings leaf-setting reachability', () => {
  const anchors = renderedSearchAnchors();

  const MOVED: Array<[string, string]> = [
    ['iOS external-link target', 'appearance:external-link-target'],
    ['Tauri in-app browser', 'appearance:in-app-browser'],
    ['Language', 'locale:language'],
    ['Timezone', 'locale:timezone'],
    ['Coding agent binaries', 'coding-agents:binaries'],
    ['Repositories', 'coding-agents:repositories'],
  ];

  it.each(MOVED)('still renders %s', (_label, anchor) => {
    expect(anchors.has(anchor)).toBe(true);
  });

  it('keeps the MCP allowlist on the MCP page, cross-linked from Permissions', () => {
    expect(anchors.has('mcp:allowed-tools')).toBe(true);
    expect(anchors.has('permissions:mcp')).toBe(true);
    expect(functionBody(SETTINGS_VIEW, 'function permissionsSection()'))
      .toContain("openSettingsSubview('mcp')");
  });

  it('resolves every search entry to a live subview and a rendered anchor', () => {
    for (const id of settingsSearchEntryIds()) {
      const entry = findSettingsEntry(id)!;
      const navKeys = SETTINGS_SUBVIEW_ITEMS.map((i) => i.key);
      expect(navKeys, `search entry "${id}" points at a dead subview`).toContain(entry.subview);
      if (entry.anchor) {
        expect(anchors.has(entry.anchor), `search entry "${id}" has no rendered anchor`).toBe(true);
      }
    }
  });

  it('keeps each search entry gated on the same platform as the row it lands on', () => {
    expect(findSettingsEntry('appearance:external-link-target')?.iosPwaOnly).toBe(true);
    expect(findSettingsEntry('appearance:in-app-browser')?.tauriOnly).toBe(true);
    expect(findSettingsEntry('debugging:autocorrect')?.iosOnly).toBe(true);
  });
});
