import { describe, it, expect } from 'vitest';
import { SETTINGS_NAV_ITEMS, SETTINGS_SYSTEM_SUBPANEL_ITEMS, settingsSubviewLabel, settingsSubviewShortLabel, migrateSettingsSubview } from './store';

describe('settings navigation', () => {
  it('keeps System last in the Workspace group and owns Thread Queue, Backup, Memory, Disk Usage, Environment Variables, and Debugging as subpanels', () => {
    const keys = SETTINGS_NAV_ITEMS.map((item) => item.key);

    // System is the last Workspace row: it is the deepest, least-often-wanted
    // category, and the "This device" group follows it.
    const workspaceKeys = SETTINGS_NAV_ITEMS.filter((i) => i.group === 'Workspace').map((i) => i.key);
    expect(workspaceKeys[workspaceKeys.length - 1]).toBe('system');

    expect(keys).not.toContain('thread-queue');
    expect(keys).not.toContain('backup');
    expect(keys).not.toContain('memory');
    expect(keys).not.toContain('disk-usage');
    expect(keys).not.toContain('environment-variables');
    expect(keys).not.toContain('debugging');
    expect(SETTINGS_SYSTEM_SUBPANEL_ITEMS.map((item) => item.key)).toEqual(['thread-queue', 'backup', 'memory', 'disk-usage', 'environment-variables', 'debugging']);
    expect(settingsSubviewLabel('thread-queue')).toBe('Thread Queue');
    expect(settingsSubviewLabel('backup')).toBe('Backup');
    expect(settingsSubviewLabel('environment-variables')).toBe('Environment Variables');
    expect(settingsSubviewLabel('debugging')).toBe('Debugging');
  });

  it('no longer carries Network access as a System subpanel: it lives in Access', () => {
    // Mobile Access and Network access are two halves of "reach this engine
    // from elsewhere", and the guide used to deep-link into the buried half.
    expect(SETTINGS_SYSTEM_SUBPANEL_ITEMS.map((i) => i.key)).not.toContain('network-access');
    expect(SETTINGS_NAV_ITEMS.map((i) => i.key)).toContain('access');
    expect(settingsSubviewLabel('access')).toBe('Access');
  });

  it('groups the home rows into contiguous runs, so a heading is emitted on change', () => {
    // SettingsView renders a group heading whenever the group differs from the
    // previous row's. A group split across two runs would render its heading
    // twice, which is why contiguity is a property of the list, not the view.
    const groups = SETTINGS_NAV_ITEMS.map((i) => i.group);
    const runs = groups.filter((g, i) => g !== groups[i - 1]);
    expect(runs).toEqual(['Assistant', 'Workspace', 'This device']);
  });

  it('labels every home row in Title Case', () => {
    // Peers used to disagree ("Network access" beside "Disk Usage"). Nav labels
    // are Title Case; section titles INSIDE a page are sentence case. A
    // shorthand is a label too, so it follows the same casing.
    for (const { label, short } of SETTINGS_NAV_ITEMS) {
      for (const name of short ? [label, short] : [label]) {
        for (const word of name.split(' ')) {
          expect(word[0], `"${name}" is not Title Case`).toBe(word[0].toUpperCase());
        }
      }
    }
  });

  it('has retired the single-setting and platform-gated categories', () => {
    // `links` held one dropdown that renders on installed iOS PWAs only;
    // `experimental` held one toggle that renders under Tauri only. Both are
    // now rows inside Appearance & Behavior → Links. `repositories` folded into
    // Coding Agents, and `mobile-access` became Access.
    const keys: string[] = SETTINGS_NAV_ITEMS.map((i) => i.key);
    for (const retired of ['links', 'experimental', 'repositories', 'mobile-access', 'network-access']) {
      expect(keys, `"${retired}" should no longer be a top-level category`).not.toContain(retired);
    }
    // `appearance` is NOT retired: the category kept its key and widened its
    // label to "Appearance & Behavior" when link routing moved in.
    expect(keys).toContain('appearance');
    expect(keys).toContain('coding-agents');
    expect(keys).toContain('locale');
  });
});

describe('the header bar’s short category names', () => {
  // The bar is the narrowest surface a category name is ever shown on. On a
  // phone the title is the one shrinkable member of a fixed-width cluster
  // between the two nav chevrons (`.header-title-cluster`,
  // styles/header-mark.css), which leaves roughly this many characters at 100%
  // UI scale and fewer above it. Past that the name ellipsises, and an ellipsis
  // on a name WE chose is a name we should have written shorter: shrinking the
  // font instead would make the bar's type jitter from screen to screen, which
  // is what both the iOS and Material top-bar conventions warn against.
  const HEADER_TITLE_MAX_CHARS = 12;
  const allItems = [...SETTINGS_NAV_ITEMS, ...SETTINGS_SYSTEM_SUBPANEL_ITEMS];

  it('every category fits the bar without an ellipsis', () => {
    for (const { key, label } of allItems) {
      const shown = settingsSubviewShortLabel(key)!;
      expect(
        shown.length,
        `"${label}" renders as "${shown}" in the header, which is too long: give it a shorter \`short\``,
      ).toBeLessThanOrEqual(HEADER_TITLE_MAX_CHARS);
    }
  });

  it('a shorthand exists only where it earns its keep', () => {
    // A `short` equal to or longer than the label is dead weight that makes the
    // bar and the Settings list disagree for nothing.
    for (const { label, short } of allItems) {
      if (short === undefined) continue;
      expect(short.length, `"${label}" has a \`short\` that is not shorter`).toBeLessThan(label.length);
      expect(short.trim(), `"${label}" has an empty \`short\``).not.toBe('');
    }
  });

  it('falls back to the full label where no shorthand is authored', () => {
    expect(settingsSubviewShortLabel('models')).toBe('Models');
    expect(settingsSubviewShortLabel('thread-queue')).toBe('Thread Queue');
  });

  it('shortens the four names that do not fit, without renaming the category', () => {
    // The full name is what the Settings list, Search Everywhere, the
    // back/forward history menu and the title's tap-tooltip keep showing.
    expect(settingsSubviewShortLabel('appearance')).toBe('Appearance');
    expect(settingsSubviewLabel('appearance')).toBe('Appearance & Behavior');
    expect(settingsSubviewShortLabel('keyboard-shortcuts')).toBe('Shortcuts');
    expect(settingsSubviewLabel('keyboard-shortcuts')).toBe('Keyboard Shortcuts');
    expect(settingsSubviewShortLabel('environment-variables')).toBe('Env Vars');
    expect(settingsSubviewLabel('environment-variables')).toBe('Environment Variables');
    expect(settingsSubviewShortLabel('coding-agents')).toBe('Agents');
    expect(settingsSubviewLabel('coding-agents')).toBe('Coding Agents');
  });

  it('reports nothing for a key that is not a category', () => {
    expect(settingsSubviewShortLabel('links' as never)).toBeUndefined();
  });
});

describe('migrateSettingsSubview', () => {
  // The persisted nav stack (`lucidos-nav-state`) survives the upgrade that
  // renames a subview, and `renderSubview` returns null for a key it no longer
  // knows. Restoring the raw string therefore lands the user on a BLANK
  // Settings panel with nothing logged, which is why this migration exists.
  it('maps every retired key onto the category that absorbed it', () => {
    expect(migrateSettingsSubview('links')).toBe('appearance');
    expect(migrateSettingsSubview('experimental')).toBe('appearance');
    expect(migrateSettingsSubview('repositories')).toBe('coding-agents');
    expect(migrateSettingsSubview('mobile-access')).toBe('access');
    expect(migrateSettingsSubview('network-access')).toBe('access');
  });

  it('passes every live key through untouched', () => {
    for (const { key } of [...SETTINGS_NAV_ITEMS, ...SETTINGS_SYSTEM_SUBPANEL_ITEMS]) {
      expect(migrateSettingsSubview(key)).toBe(key);
    }
    expect(migrateSettingsSubview('main')).toBe('main');
  });

  it('falls back to the Settings home for anything it cannot place', () => {
    // A future rename with no mapping, a truncated write, a hand-edited value:
    // the home list is the one subview that always renders.
    expect(migrateSettingsSubview('some-future-subview')).toBe('main');
    expect(migrateSettingsSubview(undefined)).toBe('main');
    expect(migrateSettingsSubview(null)).toBe('main');
    expect(migrateSettingsSubview(42)).toBe('main');
  });

  it('is not fooled by an inherited Object.prototype key', () => {
    // The key is untrusted (persisted JSON), so a plain-object lookup would
    // return a truthy `Object` / `Function` for these and hand it back as a
    // subview, landing on the blank panel this migration exists to prevent.
    for (const inherited of ['constructor', 'toString', 'valueOf', 'hasOwnProperty', '__proto__']) {
      expect(migrateSettingsSubview(inherited), `"${inherited}" must not resolve`).toBe('main');
    }
  });

  it('covers every retired key the nav list says was retired', () => {
    // Keeps the two lists honest: settings-nav.test asserts these are gone from
    // the nav, and this asserts each one still resolves somewhere renderable.
    for (const retired of ['links', 'experimental', 'repositories', 'mobile-access', 'network-access']) {
      expect(migrateSettingsSubview(retired), `"${retired}" must migrate`).not.toBe('main');
    }
  });
});
