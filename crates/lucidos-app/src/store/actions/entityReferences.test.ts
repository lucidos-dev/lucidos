import { describe, it, expect, beforeEach, vi } from 'vitest';
import { panelOverlay, pinnedApps, credentials, environmentVariables, oauthAccounts, repositories, artifacts, marketplaceCatalog } from '../store';
import type { App } from '../types';

// Mock loader functions to prevent API calls
vi.mock('./apps', () => ({ loadApps: vi.fn() }));
vi.mock('./triggers', () => ({ loadTriggers: vi.fn() }));
vi.mock('./artifacts', () => ({ loadArtifacts: vi.fn() }));
vi.mock('./credentials', () => ({ loadCredentials: vi.fn() }));
vi.mock('./environmentVariables', () => ({ loadEnvironmentVariables: vi.fn() }));
vi.mock('./oauth', () => ({ loadOAuthAccounts: vi.fn(), handleOAuthAccountConnected: vi.fn() }));
vi.mock('./repositoriesLoader', () => ({ loadRepositories: vi.fn() }));
vi.mock('./plugin-marketplaces', () => ({ refreshPluginCatalogAfterMutation: vi.fn() }));
// Partial-mock the HTTP client so the credential-event /health re-probe is
// observable without a real network call (keeps every other export real for the
// modules below that legitimately use them, e.g. pinnedApps).
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
    pendingDeviceRegistration: vi.fn(),
  };
});
// Keep removePinnedAppLocal real (the AppDeleted test asserts on its effect),
// stub loadPinnedApps so the PinnedApp* arms don't hit the network.
vi.mock('./pinnedApps', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./pinnedApps')>()),
  loadPinnedApps: vi.fn(),
}));

import { processSSEForReferences } from './entityReferences';
import { loadApps } from './apps';
import { loadTriggers } from './triggers';
import { loadArtifacts } from './artifacts';
import { loadCredentials } from './credentials';
import { loadEnvironmentVariables } from './environmentVariables';
import { loadOAuthAccounts, handleOAuthAccountConnected } from './oauth';
import { loadRepositories } from './repositoriesLoader';
import { refreshPluginCatalogAfterMutation } from './plugin-marketplaces';
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

const testApp: App = { id: 'habit-tracker', name: 'Habit Tracker', description: '' };

describe('processSSEForReferences', () => {
  beforeEach(() => {
    localStorage.clear();
    panelOverlay.value = null;
    pinnedApps.value = { status: 'loaded', data: [] };
    // Reset settings caches between tests so the gated-handler assertions
    // can't leak `status: 'loaded'` across describe blocks.
    credentials.value = { status: 'not-loaded' };
    environmentVariables.value = { status: 'not-loaded' };
    oauthAccounts.value = { status: 'not-loaded' };
    repositories.value = { status: 'not-loaded' };
    devices.value = { status: 'not-loaded' };
    artifacts.value = { status: 'not-loaded' };
    marketplaceCatalog.value = { status: 'not-loaded' };
    vi.clearAllMocks();
    // Safe default for the credential-event /health re-probe so the other
    // Credential* tests (which don't care about it) don't hit `undefined.status`.
    // `failed` leaves `llmConfigured` untouched, so it can't leak across tests.
    mockCheckHealth.mockResolvedValue({ status: 'failed', error: '' });
  });

  // ── Deleted app ──────────────────────────────────────────────────────────

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

  // ── Deleted trigger ──────────────────────────────────────────────────────

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

  // ── Trigger lifecycle events ────────────────────────────────────────────

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

  // ── AppCreated ──────────────────────────────────────────────────────────

  describe('AppCreated', () => {
    it('calls loadApps', () => {
      processSSEForReferences('AppCreated', { app_id: 'new-app', name: 'New App' });
      expect(loadApps).toHaveBeenCalled();
    });
  });

  // ── ArtifactImported ────────────────────────────────────────────────────

  describe('ArtifactImported', () => {
    it('calls loadArtifacts', () => {
      processSSEForReferences('ArtifactImported', { artifact_path: 'notes.md', source_type: 'local_file', source_detail: '/tmp/notes.md', commit_hash: 'abc' });
      expect(loadArtifacts).toHaveBeenCalled();
    });
  });

  // ── Artifact*  (gated, emitted from CC apply) ────────────────────────────
  describe('Artifact* events from change-apply', () => {
    it('does not reload when artifacts cache is not-loaded', () => {
      artifacts.value = { status: 'not-loaded' };
      processSSEForReferences('ArtifactCreated', { artifact_path: 'a.md', commit: 'c', source: 'change_apply' });
      processSSEForReferences('ArtifactUpdated', { artifact_path: 'a.md', commit: 'c', source: 'change_apply' });
      processSSEForReferences('ArtifactDeleted', { artifact_path: 'a.md', commit: 'c' });
      expect(loadArtifacts).not.toHaveBeenCalled();
    });

    it('reloads on each event when artifacts cache is loaded', () => {
      artifacts.value = { status: 'loaded', data: [] };
      processSSEForReferences('ArtifactCreated', { artifact_path: 'a.md', commit: 'c', source: 'change_apply' });
      processSSEForReferences('ArtifactUpdated', { artifact_path: 'a.md', commit: 'c', source: 'change_apply' });
      processSSEForReferences('ArtifactDeleted', { artifact_path: 'a.md', commit: 'c' });
      expect(loadArtifacts).toHaveBeenCalledTimes(3);
    });
  });

  // ── PluginMarketplace* (the live marketplace list) ───────────────────────

  describe('PluginMarketplace* events', () => {
    const loadedCatalog = {
      status: 'loaded' as const,
      data: { marketplaces: [], plugins: [], errors: [] },
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

    // The reported bug: an agent re-registered an already-registered source
    // under a new name and the open panel kept showing the old one. The rename
    // rides the same upsert event as the create, so an arm that only handled
    // "new marketplace" would leave this case broken.
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

    // The scan git-clones every registered marketplace, so a device that never
    // opened the Plugins panel must not be made to do that work.
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

  // ── Updated app with name ────────────────────────────────────────────────

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
      // localStorage.setItem should not be called when title already matches
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

  // ── ThreadTitleGenerated / ThreadTitleRenamed ─────────────────────────────

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

  // ── Graceful no-ops ──────────────────────────────────────────────────────

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

  // ── Corrupted localStorage ───────────────────────────────────────────────

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

  // ── Nav stack cursor adjustment ──────────────────────────────────────────

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

  // ── Settings-page caches (gated on `loaded`) ────────────────────────────
  // The matching loader fires only when the matching cache is already loaded
  // so cross-device events don't warm caches the user hasn't visited.

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
      // Independent of the credentials-list cache state: the backend hot-swaps
      // the active LLM provider on a credential change, so onboarding must clear
      // (or reappear) without a manual refresh. Fake timers so the delayed
      // re-check (a 600ms backstop probe) doesn't dangle past the test; we assert
      // the immediate probe (one /health call per event).
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

    // The connect half is the one that was missing: the engine wrote the
    // account row straight from the OAuth callback and emitted nothing, so
    // every device except the one running the flow sat on a stale list.
    //
    // It now routes to `handleOAuthAccountConnected` rather than reloading
    // inline, because connecting does more than refresh a list: on the device
    // that STARTED the flow it also closes the callback page, toasts, and
    // fronts the window. That handler owns the reload (and its own device
    // scoping) and is tested in oauth-connected.test.ts; what belongs here is
    // that the event reaches it, payload intact.
    it('routes a connect to the OAuth-connected handler with the payload', () => {
      oauthAccounts.value = { status: 'loaded', data: [] };
      const payload = {
        account_id: 'acc-1',
        provider: 'google',
        actor: { kind: 'device', device_id: 'device-aaa' },
      };
      processSSEForReferences('OAuthAccountConnected', payload);
      expect(handleOAuthAccountConnected).toHaveBeenCalledWith(payload);
      // And NOT a second, unscoped reload from this arm.
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
      expect(loadArtifacts).toHaveBeenCalledTimes(1);
    });

    it('RepositoryImported does not reload artifacts when that cache is not-loaded', () => {
      artifacts.value = { status: 'not-loaded' };
      processSSEForReferences('RepositoryImported', { url: 'u', branch: 'main', destination: 'd', file_count: 0, skipped_count: 0, commit: '0', files: [] });
      expect(loadArtifacts).not.toHaveBeenCalled();
    });
  });

  describe('DataFile* events', () => {
    it('reloads artifacts when an artifacts/ path changes and the cache is loaded', () => {
      artifacts.value = { status: 'loaded', data: [] };
      processSSEForReferences('DataFileWritten', { path: 'artifacts/notes.md' });
      processSSEForReferences('DataFileEdited', { path: 'artifacts/notes.md', operations_count: 1 });
      processSSEForReferences('DataFileDeleted', { path: 'artifacts/notes.md' });
      expect(loadArtifacts).toHaveBeenCalledTimes(3);
    });

    it('ignores non-artifacts paths', () => {
      artifacts.value = { status: 'loaded', data: [] };
      processSSEForReferences('DataFileWritten', { path: 'config/apis.json' });
      processSSEForReferences('DataFileEdited', { path: 'apps/foo/manifest.json', operations_count: 1 });
      expect(loadArtifacts).not.toHaveBeenCalled();
    });

    it('does not reload when artifacts cache is not-loaded', () => {
      artifacts.value = { status: 'not-loaded' };
      processSSEForReferences('DataFileWritten', { path: 'artifacts/notes.md' });
      expect(loadArtifacts).not.toHaveBeenCalled();
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
      expect(loadDevices).not.toHaveBeenCalled();
    });

    it('reloads on each event when devices cache is loaded', () => {
      devices.value = { status: 'loaded', data: [] };
      processSSEForReferences('DeviceRegistered', { device_id: 'd1' });
      processSSEForReferences('DeviceRenamed', { device_id: 'd1', name: 'phone' });
      processSSEForReferences('DevicePushChanged', { device_id: 'd1', push_enabled: true });
      processSSEForReferences('DeviceDeleted', { device_id: 'd1' });
      expect(loadDevices).toHaveBeenCalledTimes(4);
    });
  });
});
