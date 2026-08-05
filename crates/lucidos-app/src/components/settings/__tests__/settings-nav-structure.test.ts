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
      for (const m of src.matchAll(/(?:data-search-)?anchor="([^"]+)"/g)) anchors.add(m[1]);
    }
  };
  walk(COMPONENTS_DIR);
  return anchors;
}

describe('Settings nav structure', () => {
  const stripped = stripComments(SETTINGS_VIEW);

  it('renders every nav item on every platform: no category is platform-gated', () => {
    // The home list maps SETTINGS_NAV_ITEMS directly. A `.filter(...)` over it,
    // or any `key === '…'` predicate, is the regression: it gives the app a
    // different nav shape per device and hides a whole page from the platform
    // that needs it.
    expect(stripped).toMatch(/SETTINGS_NAV_ITEMS\.map\(/);
    expect(stripped).not.toMatch(/SETTINGS_NAV_ITEMS\.filter\(/);
    expect(stripped).not.toMatch(/key === '[a-z-]+'/);
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
