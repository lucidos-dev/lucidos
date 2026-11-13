import { describe, it, expect, vi } from 'vitest';
import type { InlineForm } from '../../../store/store';
import type { NavEntry } from '../../../store/actions/navigation';
import { navEntryTitle, navEntryCategory } from '../headerHelpers';

vi.mock('../../../api/client', () => ({
  listAppsApi: vi.fn().mockResolvedValue([]),
  getNotifications: vi.fn().mockResolvedValue({ notifications: [], unread_count: 0, has_more: false }),
  listCredentials: vi.fn().mockResolvedValue({ credentials: [] }),
}));

const installRequest = {
  install_id: 'i-1',
  source: 'git://example.com/habit-tracker',
  source_type: 'git' as const,
  manifest: {},
  files: [],
  overwrites: [],
  plugin_id: 'habit-tracker',
  plugin_version: '1.0.0',
  plugin_name: 'Habit Tracker',
};

const uninstallRequest = {
  uninstall_id: 'u-1',
  plugin_id: 'habit-tracker',
  plugin_version: '1.0.0',
  plugin_name: 'Habit Tracker',
  files_present: [],
  files_missing: [],
};

function entry(form: InlineForm): NavEntry {
  return {
    // `files` is the menu item the reporting user was standing on: before the
    // receipt existed, the history row for a resolved uninstall read as "Files"
    // because the overlay was dropped and only this scalar was left.
    menuItem: 'files',
    settingsSubview: 'main',
    overlay: { type: 'form', form },
    wipPreviewThreadId: null,
  };
}

describe('plugin nav-history rows', () => {
  it('names a pending panel with the action, and a receipt in the past tense', () => {
    expect(navEntryTitle(entry({ type: 'plugin-install', request: installRequest })))
      .toBe('Install Habit Tracker');
    expect(navEntryTitle(entry({
      type: 'plugin-install',
      request: installRequest,
      installed: { at: 'now', summary: 's', installed_files: [] },
    }))).toBe('Installed Habit Tracker');

    expect(navEntryTitle(entry({ type: 'plugin-uninstall', request: uninstallRequest })))
      .toBe('Uninstall Habit Tracker');
    expect(navEntryTitle(entry({
      type: 'plugin-uninstall',
      request: uninstallRequest,
      removed: { at: 'now', summary: 's', files_deleted: [], files_missing: [] },
    }))).toBe('Uninstalled Habit Tracker');
  });

  it('carries the plugin glyph rather than the settings cog it used to borrow', () => {
    expect(navEntryCategory(entry({ type: 'plugin-install', request: installRequest })))
      .toBe('plugins');
    expect(navEntryCategory(entry({ type: 'plugin-uninstall', request: uninstallRequest })))
      .toBe('plugins');
  });
});
