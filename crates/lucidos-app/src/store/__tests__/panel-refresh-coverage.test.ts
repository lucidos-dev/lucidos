import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { MENU_ITEMS, type MenuItem } from '../types';
import { SETTINGS_NAV_ITEMS, SETTINGS_SYSTEM_SUBPANEL_ITEMS, type PanelOverlay, type SettingsSubview } from '../store';
import { SETTINGS_SECTION_REFRESH } from '../../components/settings/settingsSectionRefresh';

/** How one content view takes part in the panel refresh contract. */
type Coverage =
  /** These components each call `usePanelRefresh`. Paths are under `components/`. */
  | { registers: string[] }
  /** A settings section drawn by a function, refreshed from the section table. */
  | { sectionTable: true }
  /** Shows no fetched data of its own that a refresh would re-read. */
  | { static: string };

/** Every content view, exhaustive by type: a new menu item, overlay or
 *  settings subview fails `tsc` until it has a row, and the key checks below
 *  fail the suite as well. */
const MENU: Record<MenuItem, Coverage> = {
  files: { registers: ['files/FilesView.tsx'] },
  apps: { registers: ['apps/AppsView.tsx'] },
  plugins: { registers: ['plugins/StoreTab.tsx'] },
  triggers: { registers: ['triggers/TriggersView.tsx'] },
  settings: { static: 'each subview has its own row below' },
  changes: { registers: ['changes/ChangesView.tsx'] },
  notifications: { registers: ['notifications/NotificationsView.tsx'] },
};

const OVERLAYS: Record<NonNullable<PanelOverlay>['type'], Coverage> = {
  'app-ui': { registers: ['apps/AppUiInline.tsx'] },
  'file-preview': { registers: ['files/FilePreviewInline.tsx', 'files/RepoFilePreview.tsx'] },
  'url-preview': { static: 'a web page, reloaded by its own header control' },
  'notification-detail': { static: 'rendered from the notification already in memory' },
  form: { static: 'an editor holding the user\'s draft' },
};

const SETTINGS: Record<SettingsSubview, Coverage> = {
  main: { static: 'navigation only' },
  system: { static: 'navigation only' },
  'system-overview': { registers: ['settings/SystemPage.tsx', 'settings/InstallsSection.tsx'] },
  'release-notices': { registers: ['settings/ReleaseNoticesPage.tsx'] },
  'whats-new': { registers: ['settings/WhatsNewPage.tsx'] },
  'thread-queue': { registers: ['thread-queue/ThreadQueueView.tsx'] },
  models: { sectionTable: true },
  appearance: { sectionTable: true },
  memory: { registers: ['settings/MemoryInspector.tsx'] },
  devices: { sectionTable: true },
  accounts: { sectionTable: true },
  backup: { registers: ['settings/BackupSection.tsx'] },
  'coding-agents': { sectionTable: true },
  locale: { static: 'preferences, which stay live over SSE' },
  marketplaces: { registers: ['settings/MarketplacesSection.tsx'] },
  'disk-usage': { registers: ['settings/DiskUsagePage.tsx'] },
  permissions: { registers: ['settings/AllowlistEditor.tsx'] },
  mcp: { registers: ['settings/McpServersPage.tsx'] },
  'keyboard-shortcuts': { static: 'preferences, which stay live over SSE' },
  access: { registers: ['settings/MobileAccessPage.tsx', 'settings/NetworkAccessPage.tsx'] },
  webhooks: { registers: ['settings/WebhooksPage.tsx'] },
  'environment-variables': { registers: ['settings/EnvironmentVariablesPage.tsx'] },
  debugging: { static: 'device-local switches' },
  'communication-surfaces': { static: 'sample banners drawn from local signals' },
};

const components: string = resolve(dirname(fileURLToPath(import.meta.url)), '../../components');

function check(view: string, coverage: Coverage) {
  if ('registers' in coverage) {
    for (const file of coverage.registers) {
      const src = readFileSync(resolve(components, file), 'utf-8');
      expect(src, `${view}: ${file} registers no panel refresh`).toMatch(/usePanelRefresh\(/);
    }
  } else if ('sectionTable' in coverage) {
    expect(SETTINGS_SECTION_REFRESH[view as SettingsSubview], `${view}: no row in the section table`)
      .toBeTypeOf('function');
  }
}

describe('every content panel is refreshable or listed static', () => {
  it('covers every menu item', () => {
    expect(Object.keys(MENU).sort()).toEqual([...MENU_ITEMS].sort());
    for (const [view, coverage] of Object.entries(MENU)) check(view, coverage);
  });

  it('covers every overlay', () => {
    for (const [view, coverage] of Object.entries(OVERLAYS)) check(view, coverage);
  });

  it('covers every settings subview', () => {
    const subviews = ['main', ...[...SETTINGS_NAV_ITEMS, ...SETTINGS_SYSTEM_SUBPANEL_ITEMS].map((item) => item.key)];
    expect(Object.keys(SETTINGS).sort()).toEqual([...new Set(subviews)].sort());
    for (const [view, coverage] of Object.entries(SETTINGS)) check(view, coverage);
  });

  it('lists no section in the table that the settings rows do not claim', () => {
    for (const view of Object.keys(SETTINGS_SECTION_REFRESH)) {
      expect(SETTINGS[view as SettingsSubview], view).toEqual({ sectionTable: true });
    }
  });
});
