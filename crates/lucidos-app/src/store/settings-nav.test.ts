import { describe, it, expect } from 'vitest';
import { SETTINGS_NAV_ITEMS, SETTINGS_SYSTEM_SUBPANEL_ITEMS, SETTINGS_SUBVIEW_ITEMS, settingsSubviewLabel, settingsSubviewShortLabel, migrateSettingsSubview } from './store';

describe('settings navigation', () => {
  it("keeps System last in the Workspace group and owns What's New, Thread Queue, Backup, Memory, Disk Usage, Environment Variables, and Debugging as subpanels", () => {
    const keys = SETTINGS_NAV_ITEMS.map((item) => item.key);

    const workspaceKeys = SETTINGS_NAV_ITEMS.filter((i) => i.group === 'Workspace').map((i) => i.key);
    expect(workspaceKeys[workspaceKeys.length - 1]).toBe('system');

    expect(keys).not.toContain('thread-queue');
    expect(keys).not.toContain('backup');
    expect(keys).not.toContain('memory');
    expect(keys).not.toContain('disk-usage');
    expect(keys).not.toContain('environment-variables');
    expect(keys).not.toContain('debugging');
    expect(keys).not.toContain('whats-new');
    expect(keys).not.toContain('release-notices');
    expect(keys).not.toContain('communication-surfaces');
    expect(SETTINGS_SYSTEM_SUBPANEL_ITEMS.map((item) => item.key)).toEqual(['system-overview', 'release-notices', 'whats-new', 'thread-queue', 'backup', 'memory', 'disk-usage', 'prompt-footprint', 'environment-variables', 'debugging', 'communication-surfaces']);
    expect(settingsSubviewLabel('system-overview')).toBe('Overview');
    expect(settingsSubviewLabel('release-notices')).toBe('Release Notices');
    expect(settingsSubviewShortLabel('release-notices')).toBe('Notices');
    expect(settingsSubviewLabel('whats-new')).toBe("What's New");
    expect(settingsSubviewLabel('thread-queue')).toBe('Thread Queue');
    expect(settingsSubviewLabel('backup')).toBe('Backup');
    expect(settingsSubviewLabel('environment-variables')).toBe('Environment Variables');
    expect(settingsSubviewLabel('debugging')).toBe('Debugging');
    expect(settingsSubviewLabel('communication-surfaces')).toBe('Communication Surfaces');
    expect(settingsSubviewShortLabel('communication-surfaces')).toBe('Surfaces');
  });

  it('no longer carries Network access as a System subpanel: it lives in Access', () => {
    expect(SETTINGS_SYSTEM_SUBPANEL_ITEMS.map((i) => i.key)).not.toContain('network-access');
    expect(SETTINGS_NAV_ITEMS.map((i) => i.key)).toContain('access');
    expect(settingsSubviewLabel('access')).toBe('Access');
  });

  it('groups the home rows into contiguous runs, so a heading is emitted on change', () => {
    const groups = SETTINGS_NAV_ITEMS.map((i) => i.group);
    const runs = groups.filter((g, i) => g !== groups[i - 1]);
    expect(runs).toEqual(['Assistant', 'Workspace', 'This device']);
  });

  it('labels every home row in Title Case', () => {
    for (const { label, short } of SETTINGS_NAV_ITEMS) {
      for (const name of short ? [label, short] : [label]) {
        for (const word of name.split(' ')) {
          expect(word[0], `"${name}" is not Title Case`).toBe(word[0].toUpperCase());
        }
      }
    }
  });

  it('puts MCP Servers in the Assistant group, next to Permissions', () => {
    const assistant = SETTINGS_NAV_ITEMS.filter((i) => i.group === 'Assistant').map((i) => i.key);
    expect(assistant).toContain('mcp');
    expect(assistant.indexOf('mcp')).toBe(assistant.indexOf('permissions') + 1);
    expect(settingsSubviewLabel('mcp')).toBe('MCP Servers');
  });

  it('has retired the single-setting and platform-gated categories', () => {
    const keys: string[] = SETTINGS_NAV_ITEMS.map((i) => i.key);
    for (const retired of ['links', 'experimental', 'repositories', 'mobile-access', 'network-access']) {
      expect(keys, `"${retired}" should no longer be a top-level category`).not.toContain(retired);
    }
    expect(keys).toContain('appearance');
    expect(keys).toContain('coding-agents');
    expect(keys).toContain('locale');
  });
});

describe('the header bar’s short category names', () => {
  const HEADER_TITLE_MAX_CHARS = 12;
  const allItems = SETTINGS_SUBVIEW_ITEMS;

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
  it('maps every retired key onto the category that absorbed it', () => {
    expect(migrateSettingsSubview('links')).toBe('appearance');
    expect(migrateSettingsSubview('experimental')).toBe('appearance');
    expect(migrateSettingsSubview('repositories')).toBe('coding-agents');
    expect(migrateSettingsSubview('mobile-access')).toBe('access');
    expect(migrateSettingsSubview('network-access')).toBe('access');
    expect(migrateSettingsSubview('themes')).toBe('appearance');
  });

  it('passes every live key through untouched', () => {
    for (const { key } of SETTINGS_SUBVIEW_ITEMS) {
      expect(migrateSettingsSubview(key)).toBe(key);
    }
    expect(migrateSettingsSubview('main')).toBe('main');
  });

  it('falls back to the Settings home for anything it cannot place', () => {
    expect(migrateSettingsSubview('some-future-subview')).toBe('main');
    expect(migrateSettingsSubview(undefined)).toBe('main');
    expect(migrateSettingsSubview(null)).toBe('main');
    expect(migrateSettingsSubview(42)).toBe('main');
  });

  it('is not fooled by an inherited Object.prototype key', () => {
    for (const inherited of ['constructor', 'toString', 'valueOf', 'hasOwnProperty', '__proto__']) {
      expect(migrateSettingsSubview(inherited), `"${inherited}" must not resolve`).toBe('main');
    }
  });

  it('covers every retired key the nav list says was retired', () => {
    for (const retired of ['links', 'experimental', 'repositories', 'mobile-access', 'network-access']) {
      expect(migrateSettingsSubview(retired), `"${retired}" must migrate`).not.toBe('main');
    }
  });
});
