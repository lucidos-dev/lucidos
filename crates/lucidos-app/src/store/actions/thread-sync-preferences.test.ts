import { describe, it, expect, beforeEach, vi } from 'vitest';
import { signal } from '@preact/signals-core';

// Held outside the factory so the assertions below can read them: the Backup
// page's freshness rides on which of the two a PreferencesChanged moves.
const backupStatusVersion = signal(0);
const backupPreferencesVersion = signal(0);

// Mirror the mock scaffold of the sibling thread-sync-*.test.ts files so we can
// import handleGlobalEvent in isolation.
vi.mock('../store', () => ({
  threadMap: signal(new Map()),
  focusedThreadId: signal<string | null>(null),
  changes: signal([]),
  appliedChanges: signal([]),
  changesHasMore: signal(false),
  updateAvailable: signal(false),
  applyingChangeIds: signal(new Set()),
  applyingNowThreadIds: signal(new Map()),
  generatedTitleIds: new Set(),
  codingAgentSessionVersion: signal(0),
  memoryRebuildProgress: signal(null),
  backupProgress: signal(null),
  backupStatusVersion,
  backupPreferencesVersion,
  recoveryProgress: signal(null),
  panelOverlay: signal(null),
  showConfirm: vi.fn(),
  showToast: vi.fn(),
  dismissToast: vi.fn(),
  repoSource: signal(null),
}));

vi.mock('../../api/client', () => ({ API_BASE: '', API: '/api/v1', postMcpConsent: vi.fn() }));
vi.mock('../thread-events', () => ({
  handleEvent: vi.fn(),
  isChannelDefiningEvent: vi.fn(() => false),
  makeOptimisticThreadState: vi.fn(),
  modeToInitiator: vi.fn(),
  PENDING_TITLE_PLACEHOLDER: '',
}));
vi.mock('./notifications', () => ({ handleNotificationSSE: vi.fn() }));
vi.mock('./chat-changes', () => ({ addRestartGroup: vi.fn() }));
// loadPreferences is async — the PreferencesChanged arm chains a client-update
// re-derive off it, so the mock must resolve a Promise.
vi.mock('./preferences', () => ({ loadPreferences: vi.fn(() => Promise.resolve()) }));
vi.mock('./client-update', () => ({ syncClientUpdateFromBuild: vi.fn() }));
vi.mock('./artifacts', () => ({
  loadArtifacts: vi.fn(),
  openFilePreview: vi.fn(),
  openUrl: vi.fn(),
  normalizeDataPath: vi.fn(),
}));
vi.mock('./triggers', () => ({ navigateToTrigger: vi.fn() }));
vi.mock('./apps', () => ({ refreshAppUI: vi.fn(), captureAppUI: vi.fn(), openAppById: vi.fn() }));
vi.mock('./wipPreview', () => ({ clearWipIfMatches: vi.fn() }));
vi.mock('./credentials', () => ({ openCredentialRequest: vi.fn() }));
vi.mock('./menu', () => ({
  setActiveMenu: vi.fn(),
  switchMenuItem: vi.fn(),
  openSettingsSubview: vi.fn(),
}));
vi.mock('./navigation', () => ({ pushNavState: vi.fn(), replaceNavState: vi.fn() }));
vi.mock('./push', () => ({ initPushSubscription: vi.fn() }));
vi.mock('./devices', () => ({ getDeviceId: vi.fn(), toggleDevicePush: vi.fn(), pendingDeviceRegistration: vi.fn() }));
vi.mock('../../components/chat/scrollState', () => ({ scrollToBottom: vi.fn() }));
vi.mock('./threads', () => ({ focusThread: vi.fn() }));
vi.mock('./repositories', () => ({ refreshRepoView: vi.fn(), openEncodedRepoFilePreview: vi.fn(() => false) }));
vi.mock('./entityReferences', () => ({ processSSEForReferences: vi.fn() }));
vi.mock('./thread-loading', () => ({
  loadAllThreads: vi.fn(async () => {}),
  refreshThreadEvents: vi.fn(async () => {}),
}));

const { handleGlobalEvent } = await import('./thread-sync');
const { loadPreferences } = await import('./preferences');
const { syncClientUpdateFromBuild } = await import('./client-update');

describe('handleGlobalEvent — preference events refresh the preferences cache', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    backupStatusVersion.value = 0;
    backupPreferencesVersion.value = 0;
  });

  it('PreferencesChanged reloads preferences AND re-derives the client-update surface', async () => {
    handleGlobalEvent('PreferencesChanged', {});
    expect(loadPreferences).toHaveBeenCalledTimes(1);
    // A peer may have dismissed the client-refresh toast globally; the re-derive
    // hides it on this device too. Chained off loadPreferences → runs on a
    // microtask, so wait for it.
    await vi.waitFor(() => expect(syncClientUpdateFromBuild).toHaveBeenCalledTimes(1));
  });

  // set_language / set_timezone emit LanguageSet / TimezoneSet but NOT
  // PreferencesChanged — without these arms the cached locale/timezone goes
  // stale until reload.
  it('LanguageSet reloads preferences (no client-update re-derive)', async () => {
    handleGlobalEvent('LanguageSet', { language: 'nb' });
    expect(loadPreferences).toHaveBeenCalledTimes(1);
    // The client-update re-derive is PreferencesChanged-only — a locale change
    // carries no dismissed-build change, so it must not fire a needless sw.js fetch.
    await Promise.resolve();
    expect(syncClientUpdateFromBuild).not.toHaveBeenCalled();
  });

  it('TimezoneSet reloads preferences (no client-update re-derive)', async () => {
    handleGlobalEvent('TimezoneSet', { timezone: 'Europe/Oslo' });
    expect(loadPreferences).toHaveBeenCalledTimes(1);
    await Promise.resolve();
    expect(syncClientUpdateFromBuild).not.toHaveBeenCalled();
  });
});

/**
 * The Backup page reads its provider, schedule and retention from the backup
 * endpoints, NOT from the preferences cache the arm above reloads. So a
 * PreferencesChanged that reloads the cache still leaves that page showing the
 * old destination, which is what happened live on 2026-08-07 when the agent
 * wrote `backup_provider` and the dropdown did not move until a manual reload.
 */
describe('handleGlobalEvent: a backup preference change bumps the Backup page', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    backupStatusVersion.value = 0;
    backupPreferencesVersion.value = 0;
  });

  it.each(['backup_provider', 'backup_schedule', 'backup_retention'])(
    '%s bumps the backup preferences version',
    (key) => {
      handleGlobalEvent('PreferencesChanged', { key, value: 'google_drive' });
      expect(backupPreferencesVersion.value).toBe(1);
    },
  );

  it('counts a deleted preference as a change', () => {
    // `value: null` is the engine's reset-to-default. What the page shows has to
    // move either way, so skipping it would leave the dropdown on a destination
    // the preference no longer names.
    handleGlobalEvent('PreferencesChanged', { key: 'backup_provider', value: null });
    expect(backupPreferencesVersion.value).toBe(1);
  });

  it('leaves the Backup page alone for every other key', () => {
    // The bump costs three HTTP reads, one of which resolves the provider's
    // connected / ready verdict. A theme flip must not pay for them.
    for (const key of ['theme', 'chat_model', 'language', '']) {
      handleGlobalEvent('PreferencesChanged', { key, value: 'x' });
    }
    handleGlobalEvent('PreferencesChanged', {});
    expect(backupPreferencesVersion.value).toBe(0);
  });

  it('does not refetch backup STATUS for a preference change', () => {
    // The two signals mean different things: backupStatusVersion is "a backup
    // RUN reached a terminal state" and the health card hits /backup/status on
    // it, which lists the remote folder. A retention edit must not.
    handleGlobalEvent('PreferencesChanged', { key: 'backup_retention', value: '10' });
    expect(backupStatusVersion.value).toBe(0);
  });
});
