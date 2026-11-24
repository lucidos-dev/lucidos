import { describe, it, expect, beforeEach, vi } from 'vitest';
import { panelOverlay, pinnedApps, credentials, environmentVariables, oauthAccounts, repositories, artifacts, marketplaceCatalog, marketplaceScanning, mcpServersVersion, webhooksVersion, eventHooksVersion, permissionGrantsVersion, memoryEntriesVersion } from '../store';
import type { App, PluginInstallRequest, PluginInstallReceipt } from '../types';

vi.mock('./apps', () => ({ loadApps: vi.fn() }));
vi.mock('./thread-loading', () => ({ reloadFilterFacetsIfLoaded: vi.fn() }));
vi.mock('./triggers', () => ({ loadTriggers: vi.fn(), loadHistoricalTriggers: vi.fn() }));
vi.mock('./artifacts', () => ({ loadArtifacts: vi.fn(), refreshArtifacts: vi.fn(), invalidateFilePreview: vi.fn() }));
vi.mock('./credentials', () => ({ loadCredentials: vi.fn() }));
vi.mock('./environmentVariables', () => ({ loadEnvironmentVariables: vi.fn() }));
vi.mock('./oauth', () => ({ loadOAuthAccounts: vi.fn(), handleOAuthAccountConnected: vi.fn() }));
vi.mock('./repositoriesLoader', () => ({ loadRepositories: vi.fn() }));
vi.mock('./plugin-marketplaces', () => ({ pluginCatalogScanned: vi.fn(), refreshPluginCatalogAfterMutation: vi.fn() }));
const mockCheckHealth = vi.fn();
vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  checkHealth: (...args: any[]) => mockCheckHealth(...args),
}));
vi.mock('./devices', async () => {
  const { signal } = await import('@preact/signals');
  return {
    loadDevices: vi.fn(),
    devices: signal({ status: 'not-loaded' }),
    getDeviceId: vi.fn(() => 'this-device'),
  };
});
vi.mock('./pinnedApps', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./pinnedApps')>()),
  loadPinnedApps: vi.fn(),
}));

import { processSSEForReferences } from './entityReferences';
import { loadApps } from './apps';
import { reloadFilterFacetsIfLoaded } from './thread-loading';
import { loadTriggers, loadHistoricalTriggers } from './triggers';
import { loadArtifacts, refreshArtifacts, invalidateFilePreview } from './artifacts';
import { loadCredentials } from './credentials';
import { loadEnvironmentVariables } from './environmentVariables';
import { loadOAuthAccounts, handleOAuthAccountConnected } from './oauth';
import { loadRepositories } from './repositoriesLoader';
import { pluginCatalogScanned, refreshPluginCatalogAfterMutation } from './plugin-marketplaces';
import { loadDevices, devices } from './devices';
import { loadPinnedApps } from './pinnedApps';

const RECENTS_KEY = 'lucidos-search-recents';
const NAV_KEY = 'lucidos-nav-history';

function setRecents(recents: Array<{ id: string; category: string; title: string }>): void {
  localStorage.setItem(RECENTS_KEY, JSON.stringify(recents));
}

function getRecents(): Array<{ id: string; category: string; title: string }> {
  const raw = localStorage.getItem(RECENTS_KEY);
  return raw ? JSON.parse(raw) : [];
}

function setNavStack(stack: Array<Record<string, unknown>>, cursor: number): void {
  localStorage.setItem(NAV_KEY, JSON.stringify({ stack, cursor }));
}

function getNavStack(): { stack: Array<Record<string, unknown>>; cursor: number } | null {
  const raw = localStorage.getItem(NAV_KEY);
  return raw ? JSON.parse(raw) : null;
}

const testApp: App = { id: 'habit-tracker', name: 'Habit Tracker', description: '', reveal: 'on-load', kind: 'app', reusable: false };

describe('processSSEForReferences', () => {
  beforeEach(() => {
    localStorage.clear();
    panelOverlay.value = null;
    pinnedApps.value = { status: 'loaded', data: [] };
    credentials.value = { status: 'not-loaded' };
    environmentVariables.value = { status: 'not-loaded' };
    oauthAccounts.value = { status: 'not-loaded' };
    repositories.value = { status: 'not-loaded' };
    devices.value = { status: 'not-loaded' };
    artifacts.value = { status: 'not-loaded' };
    marketplaceCatalog.value = { status: 'not-loaded' };
    vi.clearAllMocks();
    mockCheckHealth.mockResolvedValue({ status: 'failed', error: '' });
  });


  describe('an App event re-reads the filter facets, which carry app labels', () => {
    it.each(['AppCreated', 'AppUpdated', 'AppDeleted'])('%s', (type) => {
      vi.mocked(reloadFilterFacetsIfLoaded).mockClear();
      processSSEForReferences(type, { app_id: 'habit-tracker' });
      expect(reloadFilterFacetsIfLoaded).toHaveBeenCalledTimes(1);
    });
  });

  describe('AppDeleted', () => {
    it('prunes app from recents localStorage', () => {
      setRecents([
        { id: 'habit-tracker', category: 'apps', title: 'Habit Tracker' },
        { id: 'some-thread', category: 'threads', title: 'My Thread' },
      ]);
      processSSEForReferences('AppDeleted', { app_id: 'habit-tracker' });
      const recents = getRecents();
      expect(recents).toHaveLength(1);
      expect(recents[0].id).toBe('some-thread');
    });

    it('clears the app storage the deleted app kept on this device', () => {
      localStorage.setItem('appbridge:habit-tracker:app:room', 'kitchen');
      sessionStorage.setItem('appbridge:habit-tracker:sdk:scroll', '420');
      localStorage.setItem('appbridge:demo-director:app:room', 'studio');
      processSSEForReferences('AppDeleted', { app_id: 'habit-tracker' });
      expect(localStorage.getItem('appbridge:habit-tracker:app:room')).toBeNull();
      expect(sessionStorage.getItem('appbridge:habit-tracker:sdk:scroll')).toBeNull();
      expect(localStorage.getItem('appbridge:demo-director:app:room')).toBe('studio');
      localStorage.removeItem('appbridge:demo-director:app:room');
    });

    it('prunes app-ui entry from nav stack', () => {
      setNavStack([
        { overlay: { type: 'app-ui', app: { id: 'habit-tracker' } } },
        { overlay: null },
        { overlay: { type: 'app-ui', app: { id: 'other-app' } } },
      ], 2);
      processSSEForReferences('AppDeleted', { app_id: 'habit-tracker' });
      const nav = getNavStack()!;
      expect(nav.stack).toHaveLength(2);
      expect(nav.stack.every((e: Record<string, unknown>) => {
        const o = e.overlay as Record<string, unknown> | null;
        return !o || o.type !== 'app-ui' || (o.app as Record<string, unknown>).id !== 'habit-tracker';
      })).toBe(true);
    });

    it('prunes app-edit form entry from nav stack', () => {
      setNavStack([
        { overlay: { type: 'form', form: { type: 'app-edit', appId: 'habit-tracker' } } },
        { overlay: null },
      ], 1);
      processSSEForReferences('AppDeleted', { app_id: 'habit-tracker' });
      const nav = getNavStack()!;
      expect(nav.stack).toHaveLength(1);
      expect((nav.stack[0] as any).overlay).toBeNull();
    });

    it('prunes from pinned apps signal', () => {
      pinnedApps.value = { status: 'loaded', data: [{ app_id: 'habit-tracker' }, { app_id: 'other-app' }] };
      processSSEForReferences('AppDeleted', { app_id: 'habit-tracker' });
      expect(pinnedApps.value).toEqual({ status: 'loaded', data: [{ app_id: 'other-app' }] });
    });

    it('closes app-ui overlay if matching app is open', () => {
      panelOverlay.value = { type: 'app-ui', app: testApp };
      processSSEForReferences('AppDeleted', { app_id: 'habit-tracker' });
      expect(panelOverlay.value).toBeNull();
    });

    it('does not close overlay for different app', () => {
      panelOverlay.value = { type: 'app-ui', app: { ...testApp, id: 'other-app' } };
      processSSEForReferences('AppDeleted', { app_id: 'habit-tracker' });
      expect(panelOverlay.value).not.toBeNull();
      expect(panelOverlay.value!.type).toBe('app-ui');
    });

    it('calls loadApps', () => {
      processSSEForReferences('AppDeleted', { app_id: 'habit-tracker' });
      expect(loadApps).toHaveBeenCalled();
    });
  });


  describe('TriggerDeleted', () => {
    it('prunes trigger from recents', () => {
      setRecents([
        { id: 'daily-check', category: 'triggers', title: 'Daily Check' },
        { id: 'other', category: 'apps', title: 'Other' },
      ]);
      processSSEForReferences('TriggerDeleted', { trigger_id: 'daily-check' });
      const recents = getRecents();
      expect(recents).toHaveLength(1);
      expect(recents[0].id).toBe('other');
    });

    it('prunes trigger form entry from nav stack', () => {
      setNavStack([
        { overlay: { type: 'form', form: { type: 'trigger', triggerId: 'daily-check' } } },
        { overlay: null },
      ], 0);
      processSSEForReferences('TriggerDeleted', { trigger_id: 'daily-check' });
      const nav = getNavStack()!;
      expect(nav.stack).toHaveLength(1);
      expect((nav.stack[0] as any).overlay).toBeNull();
    });

    it('closes trigger form overlay if matching trigger is open', () => {
      panelOverlay.value = { type: 'form', form: { type: 'trigger', triggerId: 'daily-check' } };
      processSSEForReferences('TriggerDeleted', { trigger_id: 'daily-check' });
      expect(panelOverlay.value).toBeNull();
    });

    it('calls loadTriggers', () => {
      processSSEForReferences('TriggerDeleted', { trigger_id: 'daily-check' });
      expect(loadTriggers).toHaveBeenCalled();
    });
  });


  describe('trigger lifecycle events', () => {
    it('TriggerCreated calls loadTriggers', () => {
      processSSEForReferences('TriggerCreated', { trigger_id: 'new-trigger' });
      expect(loadTriggers).toHaveBeenCalled();
    });

    it('TriggerUpdated calls loadTriggers', () => {
      processSSEForReferences('TriggerUpdated', { trigger_id: 'daily-check' });
      expect(loadTriggers).toHaveBeenCalled();
    });

    it('TriggerEnabled calls loadTriggers', () => {
      processSSEForReferences('TriggerEnabled', { trigger_id: 'daily-check' });
      expect(loadTriggers).toHaveBeenCalled();
    });

    it('TriggerDisabled calls loadTriggers', () => {
      processSSEForReferences('TriggerDisabled', { trigger_id: 'daily-check' });
      expect(loadTriggers).toHaveBeenCalled();
    });

    it('TriggerExecuted calls loadTriggers', () => {
      processSSEForReferences('TriggerExecuted', { trigger_id: 'daily-check' });
      expect(loadTriggers).toHaveBeenCalled();
    });
  });


  describe('AppCreated', () => {
    it('calls loadApps', () => {
      processSSEForReferences('AppCreated', { app_id: 'new-app', name: 'New App' });
      expect(loadApps).toHaveBeenCalled();
    });
  });


  describe('ArtifactImported', () => {
    it('calls loadArtifacts', () => {
      processSSEForReferences('ArtifactImported', { artifact_path: 'notes.md', source_type: 'local_file', source_detail: '/tmp/notes.md', commit_hash: 'abc' });
      expect(loadArtifacts).toHaveBeenCalled();
    });
  });

  describe('Artifact* events from change-apply', () => {
    it('asks for a list refresh on each event', () => {
      processSSEForReferences('ArtifactCreated', { artifact_path: 'a.md', commit: 'c', source: 'change_apply' });
      processSSEForReferences('ArtifactUpdated', { artifact_path: 'a.md', commit: 'c', source: 'change_apply' });
      processSSEForReferences('ArtifactDeleted', { artifact_path: 'a.md', commit: 'c' });
      expect(refreshArtifacts).toHaveBeenCalledTimes(3);
      expect(loadArtifacts).not.toHaveBeenCalled();
    });

    it('offers the preview the data-relative path, not the bare artifact path', () => {
      processSSEForReferences('ArtifactUpdated', { artifact_path: 'clips/demo.mp4', commit: 'c' });
      expect(invalidateFilePreview).toHaveBeenCalledWith('artifacts/clips/demo.mp4');
    });

    it('offers the preview its path whatever the list holds', () => {
      artifacts.value = { status: 'not-loaded' };
      processSSEForReferences('ArtifactUpdated', { artifact_path: 'clips/demo.mp4', commit: 'c' });
      expect(invalidateFilePreview).toHaveBeenCalledWith('artifacts/clips/demo.mp4');
    });
  });


  describe('PluginMarketplace* events', () => {
    const loadedCatalog = {
      status: 'loaded' as const,
      data: {
        marketplaces: [],
        plugins: [],
        errors: [],
        scanned_at: '2026-09-22T10:00:00Z',
        scanning: false,
        scan_error: null,
      },
    };

    it('refreshes the catalog when a marketplace is registered', () => {
      marketplaceCatalog.value = loadedCatalog;
      processSSEForReferences('PluginMarketplaceRegistered', {
        marketplace_id: 'example-repo-1a2b3c4d',
        name: 'Example plugins',
        source: 'https://github.com/example-org/example-repo',
      });
      expect(refreshPluginCatalogAfterMutation).toHaveBeenCalledTimes(1);
    });

    it('refreshes the catalog when a marketplace is renamed', () => {
      marketplaceCatalog.value = loadedCatalog;
      processSSEForReferences('PluginMarketplaceRegistered', {
        marketplace_id: 'example-repo-1a2b3c4d',
        name: "Example's plugins",
        source: 'https://github.com/example-org/example-repo',
      });
      expect(refreshPluginCatalogAfterMutation).toHaveBeenCalledTimes(1);
    });

    it('refreshes the catalog when a marketplace is removed', () => {
      marketplaceCatalog.value = loadedCatalog;
      processSSEForReferences('PluginMarketplaceRemoved', {
        marketplace_id: 'example-repo-1a2b3c4d',
      });
      expect(refreshPluginCatalogAfterMutation).toHaveBeenCalledTimes(1);
    });

    it('does not scan when the catalog was never loaded', () => {
      marketplaceCatalog.value = { status: 'not-loaded' };
      processSSEForReferences('PluginMarketplaceRegistered', {
        marketplace_id: 'example-repo-1a2b3c4d',
        name: 'Example plugins',
        source: 'https://github.com/example-org/example-repo',
      });
      processSSEForReferences('PluginMarketplaceRemoved', {
        marketplace_id: 'example-repo-1a2b3c4d',
      });
      expect(refreshPluginCatalogAfterMutation).not.toHaveBeenCalled();
    });
  });

  describe('PluginCatalogScan* events', () => {
    beforeEach(() => {
      marketplaceScanning.value = false;
    });

    it('raises the updating cue when a scan starts', () => {
      processSSEForReferences('PluginCatalogScanStarted', {});
      expect(marketplaceScanning.value).toBe(true);
    });

    it('hands a landed scan to the catalog store, failed or not', () => {
      processSSEForReferences('PluginCatalogScanned', { failed: false });
      processSSEForReferences('PluginCatalogScanned', { failed: true });
      expect(pluginCatalogScanned).toHaveBeenCalledTimes(2);
    });
  });


  describe('AppUpdated', () => {
    it('patches recents title in localStorage', () => {
      setRecents([
        { id: 'habit-tracker', category: 'apps', title: 'Old Name' },
        { id: 'other', category: 'threads', title: 'Other' },
      ]);
      processSSEForReferences('AppUpdated', { app_id: 'habit-tracker', name: 'New Name' });
      const recents = getRecents();
      expect(recents[0].title).toBe('New Name');
      expect(recents[1].title).toBe('Other');
    });

    it('does not patch if name matches already', () => {
      setRecents([{ id: 'habit-tracker', category: 'apps', title: 'Same Name' }]);
      const before = localStorage.getItem(RECENTS_KEY);
      processSSEForReferences('AppUpdated', { app_id: 'habit-tracker', name: 'Same Name' });
      expect(localStorage.getItem(RECENTS_KEY)).toBe(before);
    });

    it('does not patch recents when name is absent', () => {
      setRecents([{ id: 'habit-tracker', category: 'apps', title: 'Old Name' }]);
      processSSEForReferences('AppUpdated', { app_id: 'habit-tracker' });
      const recents = getRecents();
      expect(recents[0].title).toBe('Old Name');
    });

    it('calls loadApps', () => {
      processSSEForReferences('AppUpdated', { app_id: 'habit-tracker', name: 'New Name' });
      expect(loadApps).toHaveBeenCalled();
    });
  });


  describe('ThreadTitleGenerated', () => {
    it('patches thread recents title', () => {
      setRecents([
        { id: 'thread-abc', category: 'threads', title: 'Old Title' },
        { id: 'other', category: 'apps', title: 'Other' },
      ]);
      processSSEForReferences('ThreadEvent', {
        thread_id: 'thread-abc',
        event: { type: 'ThreadTitleGenerated', title: 'New Title' },
      });
      const recents = getRecents();
      expect(recents[0].title).toBe('New Title');
      expect(recents[1].title).toBe('Other');
    });

    it('patches thread recents title for ThreadTitleRenamed', () => {
      setRecents([{ id: 'thread-xyz', category: 'threads', title: 'Old' }]);
      processSSEForReferences('ThreadEvent', {
        thread_id: 'thread-xyz',
        event: { type: 'ThreadTitleRenamed', title: 'Renamed' },
      });
      expect(getRecents()[0].title).toBe('Renamed');
    });

    it('ignores ThreadEvent with non-title event type', () => {
      setRecents([{ id: 'thread-abc', category: 'threads', title: 'Original' }]);
      processSSEForReferences('ThreadEvent', {
        thread_id: 'thread-abc',
        event: { type: 'MessageReceived', text: 'hello' },
      });
      expect(getRecents()[0].title).toBe('Original');
    });

    it('ignores ThreadEvent without title in event', () => {
      setRecents([{ id: 'thread-abc', category: 'threads', title: 'Original' }]);
      processSSEForReferences('ThreadEvent', {
        thread_id: 'thread-abc',
        event: { type: 'ThreadTitleGenerated' },
      });
      expect(getRecents()[0].title).toBe('Original');
    });
  });


  describe('version counters for the pages that fetch their own data', () => {
    it('bumps mcpServersVersion for every McpServer frame', () => {
      const before = mcpServersVersion.value;
      for (const type of [
        'McpServerRegistered',
        'McpServerUpdated',
        'McpServerRemoved',
        'McpServerDisabledToolsChanged',
      ]) {
        processSSEForReferences(type, {});
      }
      expect(mcpServersVersion.value).toBe(before + 4);
    });

    it('bumps webhooksVersion for every Webhook frame', () => {
      const before = webhooksVersion.value;
      for (const type of ['WebhookCreated', 'WebhookUpdated', 'WebhookDeleted']) {
        processSSEForReferences(type, {});
      }
      expect(webhooksVersion.value).toBe(before + 3);
    });

    it('bumps eventHooksVersion for every arming frame', () => {
      const before = eventHooksVersion.value;
      for (const type of ['EventHookArmed', 'EventHookDisarmed', 'EventHookRemoved']) {
        processSSEForReferences(type, { slug: 'deploy-guard' });
      }
      expect(eventHooksVersion.value).toBe(before + 3);
    });

    it('bumps memoryEntriesVersion when a memory is corrected', () => {
      const before = memoryEntriesVersion.value;
      processSSEForReferences('MemoryCorrected', { wrong_fact: 'x', removed: [] });
      expect(memoryEntriesVersion.value).toBe(before + 1);
    });

    it('bumps permissionGrantsVersion when a grant file changes', () => {
      const before = permissionGrantsVersion.value;
      processSSEForReferences('PermissionGrantsChanged', { grant_file: 'cc-allowed-tools' });
      expect(permissionGrantsVersion.value).toBe(before + 1);
    });

    it('leaves the counters alone for an unrelated frame', () => {
      const counters = () => [
        mcpServersVersion.value, webhooksVersion.value, permissionGrantsVersion.value,
        memoryEntriesVersion.value, eventHooksVersion.value,
      ];
      const before = counters();
      processSSEForReferences('AppCreated', { app_id: 'x' });
      expect(counters()).toEqual(before);
    });
  });

  describe('a plugin request panel a peer resolved', () => {
    const request: PluginInstallRequest = {
      install_id: 'staged-1',
      plugin_id: 'habit-tracker',
      plugin_version: '1.0.0',
      plugin_name: 'Habit Tracker',
      source: 'https://example.com/example-repo',
      source_type: 'git',
      manifest: {},
      files: [],
      overwrites: [],
    };
    const receipt: PluginInstallReceipt = {
      at: '2026-01-01T00:00:00Z',
      summary: 'installed',
      installed_files: [],
    };

    function openInstallPanel(installed?: PluginInstallReceipt): void {
      panelOverlay.value = { type: 'form', form: { type: 'plugin-install', request, installed } };
    }

    it('closes when the peer confirmed', () => {
      openInstallPanel();
      processSSEForReferences('PluginInstalled', { id: 'habit-tracker' });
      expect(panelOverlay.value).toBeNull();
    });

    it('closes on a row written before the frame carried a top-level id', () => {
      openInstallPanel();
      processSSEForReferences('PluginInstalled', {
        manifest: { manifest: { id: 'habit-tracker' } },
      });
      expect(panelOverlay.value).toBeNull();
    });

    it('closes when the peer cancelled', () => {
      openInstallPanel();
      processSSEForReferences('PluginInstallCanceled', { id: 'habit-tracker' });
      expect(panelOverlay.value).toBeNull();
    });

    it('leaves a panel for a different plugin alone', () => {
      openInstallPanel();
      processSSEForReferences('PluginInstallCanceled', { id: 'demo-director' });
      expect(panelOverlay.value).not.toBeNull();
    });

    it('leaves the panel alone when THIS device is the actor', () => {
      openInstallPanel();
      processSSEForReferences('PluginInstalled', {
        id: 'habit-tracker',
        actor: { kind: 'device', device_id: 'this-device' },
      });
      expect(panelOverlay.value).not.toBeNull();
    });

    it('closes when another device is the actor', () => {
      openInstallPanel();
      processSSEForReferences('PluginInstalled', {
        id: 'habit-tracker',
        actor: { kind: 'device', device_id: 'some-other-device' },
      });
      expect(panelOverlay.value).toBeNull();
    });

    it('leaves a receipt standing, actor or not', () => {
      openInstallPanel(receipt);
      processSSEForReferences('PluginInstalled', { id: 'habit-tracker' });
      expect(panelOverlay.value).not.toBeNull();
    });

    it('does not mistake an uninstall frame for the install panel', () => {
      openInstallPanel();
      processSSEForReferences('PluginUninstalled', { id: 'habit-tracker' });
      expect(panelOverlay.value).not.toBeNull();
    });
  });

  describe('historical triggers', () => {
    it('reloads the historical registry when a trigger is deleted', () => {
      processSSEForReferences('TriggerDeleted', { trigger_id: 'gone' });
      expect(loadHistoricalTriggers).toHaveBeenCalled();
    });

    it('leaves it alone for an update, which cannot change membership', () => {
      processSSEForReferences('TriggerUpdated', { trigger_id: 'still-here' });
      expect(loadHistoricalTriggers).not.toHaveBeenCalled();
    });
  });


  describe('graceful no-ops', () => {
    it('does not crash for unknown SSE type', () => {
      expect(() => processSSEForReferences('UnknownEvent', { foo: 'bar' })).not.toThrow();
    });

    it('does not crash when ThreadEvent has no event field', () => {
      expect(() => processSSEForReferences('ThreadEvent', { thread_id: 'abc' })).not.toThrow();
    });

    it('does not crash when ThreadEvent has no thread_id', () => {
      expect(() => processSSEForReferences('ThreadEvent', { event: { type: 'ThreadTitleGenerated', title: 'x' } })).not.toThrow();
    });

    it('does not crash for AppDeleted with missing app_id', () => {
      expect(() => processSSEForReferences('AppDeleted', {})).not.toThrow();
    });

    it('does not crash for TriggerDeleted with missing trigger_id', () => {
      expect(() => processSSEForReferences('TriggerDeleted', {})).not.toThrow();
    });
  });


  describe('corrupted localStorage', () => {
    it('does not crash when recents is invalid JSON', () => {
      localStorage.setItem(RECENTS_KEY, '{{not json');
      expect(() => processSSEForReferences('AppDeleted', { app_id: 'x' })).not.toThrow();
    });

    it('does not crash when nav stack is invalid JSON', () => {
      localStorage.setItem(NAV_KEY, '{{not json');
      expect(() => processSSEForReferences('AppDeleted', { app_id: 'x' })).not.toThrow();
    });

    it('does not crash when nav stack has no array', () => {
      localStorage.setItem(NAV_KEY, JSON.stringify({ stack: 'not-array', cursor: 0 }));
      expect(() => processSSEForReferences('AppDeleted', { app_id: 'x' })).not.toThrow();
    });
  });


  describe('nav stack cursor adjustment', () => {
    it('adjusts cursor when pruned entry is before cursor', () => {
      setNavStack([
        { overlay: { type: 'app-ui', app: { id: 'habit-tracker' } } },
        { overlay: null },
        { overlay: { type: 'file-preview', path: 'notes.md' } },
      ], 2);
      processSSEForReferences('AppDeleted', { app_id: 'habit-tracker' });
      const nav = getNavStack()!;
      expect(nav.stack).toHaveLength(2);
      expect(nav.cursor).toBe(1);
    });

    it('does not adjust cursor when pruned entry is after cursor', () => {
      setNavStack([
        { overlay: null },
        { overlay: { type: 'app-ui', app: { id: 'habit-tracker' } } },
      ], 0);
      processSSEForReferences('AppDeleted', { app_id: 'habit-tracker' });
      const nav = getNavStack()!;
      expect(nav.stack).toHaveLength(1);
      expect(nav.cursor).toBe(0);
    });
  });


  describe('Credential* events', () => {
    it('does not reload when credentials cache is not-loaded', () => {
      credentials.value = { status: 'not-loaded' };
      processSSEForReferences('CredentialCreated', { service_name: 'openai' });
      processSSEForReferences('CredentialUpdated', { service_name: 'openai' });
      processSSEForReferences('CredentialDeleted', { service_name: 'openai' });
      expect(loadCredentials).not.toHaveBeenCalled();
    });

    it('reloads on each event when credentials cache is loaded', () => {
      credentials.value = { status: 'loaded', data: [] };
      processSSEForReferences('CredentialCreated', { service_name: 'openai' });
      processSSEForReferences('CredentialUpdated', { service_name: 'openai' });
      processSSEForReferences('CredentialDeleted', { service_name: 'openai' });
      expect(loadCredentials).toHaveBeenCalledTimes(3);
    });

    it('re-probes /health on each event so llmConfigured reflects a runtime provider swap', () => {
      vi.useFakeTimers();
      mockCheckHealth.mockResolvedValue({ status: 'loaded', data: { llm_configured: true } });
      credentials.value = { status: 'not-loaded' };
      processSSEForReferences('CredentialCreated', { service_name: 'openai' });
      processSSEForReferences('CredentialUpdated', { service_name: 'openai' });
      processSSEForReferences('CredentialDeleted', { service_name: 'openai' });
      expect(mockCheckHealth).toHaveBeenCalledTimes(3);
      vi.useRealTimers();
    });
  });

  describe('EnvironmentVariable* events', () => {
    it('does not reload when env vars cache is not-loaded', () => {
      environmentVariables.value = { status: 'not-loaded' };
      processSSEForReferences('EnvironmentVariableSet', { name: 'LUCIDOS_REPO', value: 'x' });
      processSSEForReferences('EnvironmentVariableDeleted', { name: 'LUCIDOS_REPO' });
      expect(loadEnvironmentVariables).not.toHaveBeenCalled();
    });

    it('reloads on each event when env vars cache is loaded', () => {
      environmentVariables.value = { status: 'loaded', data: [] };
      processSSEForReferences('EnvironmentVariableSet', { name: 'LUCIDOS_REPO', value: 'x' });
      processSSEForReferences('EnvironmentVariableDeleted', { name: 'LUCIDOS_REPO' });
      expect(loadEnvironmentVariables).toHaveBeenCalledTimes(2);
    });
  });

  describe('OAuthAccount* events', () => {
    it('does not reload when oauthAccounts cache is not-loaded', () => {
      oauthAccounts.value = { status: 'not-loaded' };
      processSSEForReferences('OAuthAccountDeleted', { account_id: 'acc-1' });
      expect(loadOAuthAccounts).not.toHaveBeenCalled();
    });

    it('reloads when oauthAccounts cache is loaded', () => {
      oauthAccounts.value = { status: 'loaded', data: [] };
      processSSEForReferences('OAuthAccountDeleted', { account_id: 'acc-1' });
      expect(loadOAuthAccounts).toHaveBeenCalled();
    });

    it('routes a connect to the OAuth-connected handler with the payload', () => {
      oauthAccounts.value = { status: 'loaded', data: [] };
      const payload = {
        account_id: 'acc-1',
        provider: 'google',
        actor: { kind: 'device', device_id: 'device-aaa' },
      };
      processSSEForReferences('OAuthAccountConnected', payload, 'oauth-envelope');
      expect(handleOAuthAccountConnected).toHaveBeenCalledWith(payload, 'oauth-envelope');
      expect(loadOAuthAccounts).not.toHaveBeenCalled();
    });
  });

  describe('Repository* events', () => {
    it('does not reload when repositories cache is not-loaded', () => {
      repositories.value = { status: 'not-loaded' };
      processSSEForReferences('RepositoryAdded', { repo_id: 'r1', name: 'r', root_path: '/' });
      processSSEForReferences('RepositoryRemoved', { repo_id: 'r1' });
      expect(loadRepositories).not.toHaveBeenCalled();
    });

    it('reloads the registered-repos list only on Add/Remove (not Imported)', () => {
      repositories.value = { status: 'loaded', data: [] };
      processSSEForReferences('RepositoryAdded', { repo_id: 'r1', name: 'r', root_path: '/' });
      processSSEForReferences('RepositoryRemoved', { repo_id: 'r1' });
      expect(loadRepositories).toHaveBeenCalledTimes(2);
    });

    it('RepositoryImported refreshes artifacts (a clone into data/artifacts/), not the repo list', () => {
      repositories.value = { status: 'loaded', data: [] };
      artifacts.value = { status: 'loaded', data: [] };
      processSSEForReferences('RepositoryImported', { url: 'u', branch: 'main', destination: 'd', file_count: 0, skipped_count: 0, commit: '0', files: [] });
      expect(loadRepositories).not.toHaveBeenCalled();
      expect(refreshArtifacts).toHaveBeenCalledTimes(1);
    });
  });

  describe('DataFile* events', () => {
    it('asks for a list refresh when an artifacts/ path changes', () => {
      processSSEForReferences('DataFileWritten', { path: 'artifacts/notes.md' });
      processSSEForReferences('DataFileEdited', { path: 'artifacts/notes.md', operations_count: 1 });
      processSSEForReferences('DataFileDeleted', { path: 'artifacts/notes.md' });
      expect(refreshArtifacts).toHaveBeenCalledTimes(3);
    });

    it('leaves the artifacts LIST alone for a non-artifacts path', () => {
      artifacts.value = { status: 'loaded', data: [] };
      processSSEForReferences('DataFileWritten', { path: 'config/apis.json' });
      processSSEForReferences('DataFileEdited', { path: 'apps/foo/manifest.json', operations_count: 1 });
      expect(refreshArtifacts).not.toHaveBeenCalled();
    });

    it('offers the preview every data path, artifacts or not', () => {
      artifacts.value = { status: 'loaded', data: [] };
      processSSEForReferences('DataFileWritten', { path: 'config/apis.json' });
      processSSEForReferences('DataFileEdited', { path: 'knowhow/ops/deploy.md', operations_count: 1 });
      processSSEForReferences('DataFileDeleted', { path: 'apps/demo/old.html' });
      expect(invalidateFilePreview).toHaveBeenCalledWith('config/apis.json');
      expect(invalidateFilePreview).toHaveBeenCalledWith('knowhow/ops/deploy.md');
      expect(invalidateFilePreview).toHaveBeenCalledWith('apps/demo/old.html');
    });
  });

  describe('PinnedApp* events', () => {
    it('reloads pinned apps when the event targets THIS device', () => {
      processSSEForReferences('PinnedAppPinned', { app_id: 'a', device_id: 'this-device' });
      processSSEForReferences('PinnedAppUnpinned', { app_id: 'a', device_id: 'this-device' });
      expect(loadPinnedApps).toHaveBeenCalledTimes(2);
    });

    it('ignores events for a different device', () => {
      processSSEForReferences('PinnedAppPinned', { app_id: 'a', device_id: 'other-device' });
      processSSEForReferences('PinnedAppUnpinned', { app_id: 'a', device_id: 'other-device' });
      expect(loadPinnedApps).not.toHaveBeenCalled();
    });
  });

  describe('Device* events', () => {
    it('does not reload when devices cache is not-loaded', () => {
      devices.value = { status: 'not-loaded' };
      processSSEForReferences('DeviceRegistered', { device_id: 'd1' });
      processSSEForReferences('DeviceRenamed', { device_id: 'd1', name: 'phone' });
      processSSEForReferences('DevicePushChanged', { device_id: 'd1', push_enabled: true });
      processSSEForReferences('DeviceDeleted', { device_id: 'd1' });
      processSSEForReferences('DeviceHandedOver', { device_id: 'd2', old_device_id: 'd1' });
      expect(loadDevices).not.toHaveBeenCalled();
    });

    it('reloads on each event when devices cache is loaded', () => {
      devices.value = { status: 'loaded', data: [] };
      processSSEForReferences('DeviceRegistered', { device_id: 'd1' });
      processSSEForReferences('DeviceRenamed', { device_id: 'd1', name: 'phone' });
      processSSEForReferences('DevicePushChanged', { device_id: 'd1', push_enabled: true });
      processSSEForReferences('DeviceDeleted', { device_id: 'd1' });
      processSSEForReferences('DeviceHandedOver', { device_id: 'd2', old_device_id: 'd1' });
      expect(loadDevices).toHaveBeenCalledTimes(5);
    });
  });
});
