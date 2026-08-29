/**
 * The Settings information architecture, guarded at the level the bugs actually
 * happen: PLACEMENT and REACHABILITY, not predicates.
 *
 * Replaces `external-links-row-reachable.test.ts`, which pinned one symptom of a
 * general fault. The bug there: the external-link-target row's own visibility
 * predicate (`externalLinkTargetConfigurable`, iOS-PWA-only) was correct and
 * unit-tested, and the row was still unreachable, because it rendered inside the
 * **Experimental** subview whose nav entry was filtered to `isTauri()`. Nothing
 * failed. On a desktop browser the predicate hid the row; on an installed iOS
 * PWA the nav entry hid the whole subview. The setting existed and no user could
 * open it.
 *
 * The fix at the time was a new top-level `Links` category, itself gated on the
 * same iOS predicate, which moved the fault rather than removing it. The
 * structural fix is the rule this file enforces: **no top-level Settings
 * category is platform-gated**; gating lives on a row or section inside one, so
 * an absent control just means one fewer row on a page that still has others.
 *
 * A predicate test cannot catch any of this: it asks "would this row render?",
 * never "can anyone navigate to where it renders?".
 *
 * Source-scan rather than a mounted render: `SettingsView` pulls in the whole
 * store, the model registry, OAuth and device state, so standing it up to
 * observe one section's position would pin the mechanism instead of the
 * requirement (the same reasoning as `useStartup.test.ts`).
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, join } from 'node:path';
import { SETTINGS_NAV_ITEMS, SETTINGS_SYSTEM_SUBPANEL_ITEMS } from '../../../store/store';
import { findSettingsEntry, settingsSearchEntryIds } from '../../search/searchIndex';

const here = dirname(fileURLToPath(import.meta.url));
const SETTINGS_VIEW = readFileSync(resolve(here, '..', 'SettingsView.tsx'), 'utf8');
const SYSTEM_SUBMENU = readFileSync(resolve(here, '..', 'SystemSubmenu.tsx'), 'utf8');
const NAV_ROW = readFileSync(resolve(here, '..', 'SettingsNavRow.tsx'), 'utf8');
const COMPONENTS_DIR = resolve(here, '..', '..');

/** Strip comments so the prose explaining a call can never stand in for it. */
function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^\\:])\/\/.*$/gm, '$1');
}

/** The body of a `function <name>()` declaration, by brace matching. */
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

/** Every anchor value rendered anywhere under `components/`. The anchors live
 *  across a dozen files (SettingsView, SystemPage's panels, LocaleSection,
 *  MobileAccessPage, …), so the sweep is by tree, not by list: a list would be
 *  the very thing that goes stale.
 *
 *  Two spellings, because two components own the attribute. Most sites write
 *  `data-search-anchor="…"` inline; `AllowlistEditor` takes the value as an
 *  `anchor` PROP and renders `data-search-anchor={props.anchor}`, so the
 *  literal only ever appears at the call site. Missing the second spelling
 *  would report the two Permissions allowlists as unrendered. */
function renderedAnchors(): Set<string> {
  const anchors = new Set<string>();
  const walk = (dir: string) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) { walk(path); continue; }
      if (!entry.name.endsWith('.tsx')) continue;
      const src = readFileSync(path, 'utf8');
      // Any prop whose name ends in `anchor`, so a component that renders the
      // attribute on the caller's behalf still counts. `ModelSelectionRow` is
      // one, taking the anchor for the row it draws. Over-matching an unrelated
      // `*Anchor` prop only adds a name to the set, and the set is read with
      // `.has`.
      for (const m of src.matchAll(/\b[\w-]*[Aa]nchor="([^"]+)"/g)) anchors.add(m[1]);
    }
  };
  walk(COMPONENTS_DIR);
  return anchors;
}

/** No `key` comparison stands between a list's `.map(` and the row it renders.
 *
 *  That is where a platform gate would go, and a gated row hides a whole page
 *  from the device that needs it. A decoration the row hangs off its own label
 *  is not one. The slice ends at the element name, so a `badge={key === …}`
 *  prop stays out of scope: the System attention badge picks its category that
 *  way and hides nothing. */
function expectRowGatedOnNothing(src: string, mapCall: string): void {
  const map = src.indexOf(mapCall);
  const row = src.indexOf('<SettingsNavRow', map);
  expect(map, `\`${mapCall}\``).toBeGreaterThan(-1);
  expect(row, 'the nav row element').toBeGreaterThan(map);
  expect(src.slice(map, row)).not.toMatch(/key === '[a-z-]+'/);
  expect(src.slice(map, row)).not.toMatch(/key !== '[a-z-]+'/);
}

describe('Settings nav structure', () => {
  const stripped = stripComments(SETTINGS_VIEW);

  it('renders every nav item on every platform: no category is platform-gated', () => {
    // The home list maps SETTINGS_NAV_ITEMS directly. A `.filter(...)` over it,
    // or a predicate deciding whether the ROW renders, is the regression: it
    // gives the app a different nav shape per device and hides a whole page
    // from the platform that needs it.
    expect(stripped).toMatch(/SETTINGS_NAV_ITEMS\.map\(/);
    expect(stripped).not.toMatch(/SETTINGS_NAV_ITEMS\.filter\(/);
    expectRowGatedOnNothing(stripped, 'SETTINGS_NAV_ITEMS.map(');
  });

  it('lists every System sub-page in the submenu, gated on nothing', () => {
    // The System submenu is the only way into a sub-page, so it maps
    // SETTINGS_SYSTEM_SUBPANEL_ITEMS directly. Same rule as the home list
    // above, and the same regression: a filter or a key comparison in front of
    // the row hides a page from whoever needs it.
    const submenu = stripComments(SYSTEM_SUBMENU);
    expect(submenu).toMatch(/SETTINGS_SYSTEM_SUBPANEL_ITEMS\.map\(/);
    expect(submenu).not.toMatch(/SETTINGS_SYSTEM_SUBPANEL_ITEMS\.filter\(/);
    expectRowGatedOnNothing(submenu, 'SETTINGS_SYSTEM_SUBPANEL_ITEMS.map(');
  });

  it('opens every drilldown row with a real button, in the one row both lists share', () => {
    // Reachability is the BUTTON: a clickable div puts the page it opens, and
    // every control on it, out of keyboard reach. Both lists render
    // `SettingsNavRow`, so the check lives where the element does rather than
    // once per caller.
    const row = stripComments(NAV_ROW);
    expect(row).toContain('<button');
    expect(row).toContain('type="button"');
    expect(row).toContain('class="settings-section-title settings-nav-row"');
  });

  it('has a renderSubview case for every nav key, so no row opens onto nothing', () => {
    const body = functionBody(SETTINGS_VIEW, 'function renderSubview()');
    for (const { key } of [...SETTINGS_NAV_ITEMS, ...SETTINGS_SYSTEM_SUBPANEL_ITEMS]) {
      expect(body, `renderSubview has no case for '${key}'`).toContain(`case '${key}':`);
    }
  });

  it('keeps both link settings in one section inside Appearance & Behavior, gated per ROW', () => {
    // One user question ("where does a link open?"), one section, two
    // platform-conditional rows. Splitting them back into two categories is
    // what the top comment describes.
    // Assert the predicates GATE their rows, not merely that they are called:
    // computing `showExternalTarget` and then dropping the `&&` wrapper would
    // satisfy a call-site check while rendering the iOS-only dropdown
    // everywhere. That is the shape the deleted
    // `external-links-row-reachable.test.ts` pinned via its early return.
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
    // linksSection may return null (neither row applies), which is only safe
    // because Appearance & Behavior renders Theme + Typography
    // unconditionally. If those ever become conditional, the page can come up
    // empty.
    const iface = functionBody(SETTINGS_VIEW, 'function appearanceSection()');
    expect(functionBody(SETTINGS_VIEW, 'function linksSection()')).toContain('return null');
    // Theme and Typography carry no guard: they are what keeps the page from
    // being empty when neither link row applies.
    for (const anchor of ['appearance:theme', 'appearance:typography']) {
      const guarded = new RegExp(`\\S\\s*&&\\s*\\(\\s*<div class="settings-section">\\s*<div class="settings-section-title" data-search-anchor="${anchor}"`);
      expect(iface).toContain(`data-search-anchor="${anchor}"`);
      expect(iface, `${anchor} must render unconditionally`).not.toMatch(guarded);
    }
  });
});

describe('Settings leaf-setting reachability', () => {
  const anchors = renderedAnchors();

  // Each setting that MOVED in the 2026-08-05 restructure, with the anchor it
  // must still render under. A moved control that renders nowhere is exactly
  // the failure this suite exists for, and it is silent.
  const MOVED: Array<[string, string]> = [
    ['iOS external-link target', 'appearance:external-link-target'],
    ['Tauri in-app browser', 'appearance:in-app-browser'],
    ['Language', 'locale:language'],
    ['Timezone', 'locale:timezone'],
    ['Coding agent binaries', 'coding-agents:binaries'],
    ['Repositories', 'coding-agents:repositories'],
    ['Network bind', 'access:network'],
  ];

  it.each(MOVED)('still renders %s', (_label, anchor) => {
    expect(anchors.has(anchor)).toBe(true);
  });

  it('keeps the MCP allowlist on the MCP page, cross-linked from Permissions', () => {
    // The allowlist is meaningless without the server list beside it: a
    // pattern names a server and a tool. Permissions therefore points at the
    // page rather than holding a third editor. The pointer is a real
    // navigation, not a sentence telling the user to go looking.
    expect(anchors.has('mcp:allowed-tools')).toBe(true);
    expect(anchors.has('permissions:mcp')).toBe(true);
    expect(functionBody(SETTINGS_VIEW, 'function permissionsSection()'))
      .toContain("openSettingsSubview('mcp')");
  });

  it('resolves every search entry to a live subview and a rendered anchor', () => {
    for (const id of settingsSearchEntryIds()) {
      const entry = findSettingsEntry(id)!;
      const navKeys = [...SETTINGS_NAV_ITEMS, ...SETTINGS_SYSTEM_SUBPANEL_ITEMS].map((i) => i.key);
      expect(navKeys, `search entry "${id}" points at a dead subview`).toContain(entry.subview);
      if (entry.anchor) {
        expect(anchors.has(entry.anchor), `search entry "${id}" has no rendered anchor`).toBe(true);
      }
    }
  });

  it('keeps each search entry gated on the same platform as the row it lands on', () => {
    // The flags exist so search never offers a result that lands on nothing.
    // These two rows are the only platform-conditional ones left in Settings.
    expect(findSettingsEntry('appearance:external-link-target')?.iosPwaOnly).toBe(true);
    expect(findSettingsEntry('appearance:in-app-browser')?.tauriOnly).toBe(true);
  });
});
