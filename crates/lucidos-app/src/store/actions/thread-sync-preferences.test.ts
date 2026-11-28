import { describe, it, expect, beforeEach, vi } from 'vitest';
import { signal } from '@preact/signals-core';

const backupStatusVersion = signal(0);
const backupPreferencesVersion = signal(0);

vi.mock('../store', () => ({
  threadMap: signal(new Map()),
  focusedThreadId: signal<string | null>(null),
  promptFootprintVersion: signal(0),
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
  backupRecipientVersion: signal(0),
  intelligenceLadderVersion: signal(0),
  chatModels: signal({ status: 'not-loaded' }),
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
vi.mock('./preferences', () => ({
  loadPreferences: vi.fn(() => Promise.resolve()),
  refreshActiveTheme: vi.fn(() => Promise.resolve()),
}));
vi.mock('./engine-update', () => ({ handleEngineVersionStatusChanged: vi.fn() }));
vi.mock('./client-update', () => ({ syncClientUpdateFromBuild: vi.fn() }));
vi.mock('./releaseNotices', () => ({ loadReleaseNotices: vi.fn() }));
vi.mock('./gatewayUsers', () => ({ loadGatewayUsers: vi.fn() }));
vi.mock('./reader-devices', () => ({ loadReaderDevices: vi.fn(), readIsMine: vi.fn(() => true) }));
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
  openBackupSettings: vi.fn(),
}));
vi.mock('./navigation', () => ({ pushNavState: vi.fn(), replaceNavState: vi.fn() }));
vi.mock('./push', () => ({ setDevicePushEnabled: vi.fn() }));
vi.mock('./devices', () => ({ getDeviceId: vi.fn() }));
vi.mock('./threads', () => ({ focusThread: vi.fn() }));
vi.mock('./repositories', () => ({ refreshRepoView: vi.fn(), openEncodedRepoFilePreview: vi.fn(() => false) }));
vi.mock('./entityReferences', () => ({
  processSSEForReferences: vi.fn(),
  refreshLlmConfigured: vi.fn(),
  PROVIDER_PREFERENCE_KEYS: new Set(['opencode_free_enabled', 'provider_enabled_openai']),
}));
vi.mock('./thread-loading', () => ({
  loadAllThreads: vi.fn(async () => {}),
  refreshThreadEvents: vi.fn(async () => {}),
}));

const { handleGlobalEvent } = await import('./thread-sync');
const { handleEngineVersionStatusChanged } = await import('./engine-update');
const { loadPreferences } = await import('./preferences');
const { syncClientUpdateFromBuild } = await import('./client-update');
const { refreshLlmConfigured } = await import('./entityReferences');
const { loadReleaseNotices } = await import('./releaseNotices');
const { loadGatewayUsers } = await import('./gatewayUsers');
const { loadReaderDevices } = await import('./reader-devices');

describe('handleGlobalEvent — preference events refresh the preferences cache', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    backupStatusVersion.value = 0;
    backupPreferencesVersion.value = 0;
  });

  it('PreferencesChanged reloads preferences AND re-derives the client-update surface', async () => {
    handleGlobalEvent('PreferencesChanged', {});
    expect(loadPreferences).toHaveBeenCalledTimes(1);
    await vi.waitFor(() => expect(syncClientUpdateFromBuild).toHaveBeenCalledTimes(1));
  });

  it.each(['theme', 'chat_model', 'client_refresh_dismissed_build', 'engine_switch_dismissed_build'])('does not read engine status for %s', async (key) => {
    handleGlobalEvent('PreferencesChanged', { key, value: 'example' });
    await vi.waitFor(() => expect(syncClientUpdateFromBuild).toHaveBeenCalledTimes(1));
    expect(handleEngineVersionStatusChanged).not.toHaveBeenCalled();
  });

  it('LanguageSet reloads preferences (no client-update re-derive)', async () => {
    handleGlobalEvent('LanguageSet', { language: 'nb' });
    expect(loadPreferences).toHaveBeenCalledTimes(1);
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

describe('handleGlobalEvent: a preference change re-reads the release notices', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it.each(['memory_module', 'theme-mode'])('%s re-reads them', (key) => {
    handleGlobalEvent('PreferencesChanged', { key, value: 'tree' });
    expect(loadReleaseNotices).toHaveBeenCalledTimes(1);
  });
});

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
    handleGlobalEvent('PreferencesChanged', { key: 'backup_provider', value: null });
    expect(backupPreferencesVersion.value).toBe(1);
  });

  it('leaves the Backup page alone for every other key', () => {
    for (const key of ['theme-mode', 'chat_model', 'language', '']) {
      handleGlobalEvent('PreferencesChanged', { key, value: 'x' });
    }
    handleGlobalEvent('PreferencesChanged', {});
    expect(backupPreferencesVersion.value).toBe(0);
  });

  it('does not refetch backup STATUS for a preference change', () => {
    handleGlobalEvent('PreferencesChanged', { key: 'backup_retention', value: '10' });
    expect(backupStatusVersion.value).toBe(0);
  });
});

describe('handleGlobalEvent: a provider preference re-probes /health', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it.each(['opencode_free_enabled', 'provider_enabled_openai'])(
    '%s re-probes',
    (key) => {
      handleGlobalEvent('PreferencesChanged', { key, value: 'false' });
      expect(refreshLlmConfigured).toHaveBeenCalledTimes(1);
    },
  );

  it('leaves /health alone for every other key', () => {
    for (const key of ['theme-mode', 'chat_model', '']) {
      handleGlobalEvent('PreferencesChanged', { key, value: 'x' });
    }
    handleGlobalEvent('PreferencesChanged', {});
    expect(refreshLlmConfigured).not.toHaveBeenCalled();
  });
});

describe('handleGlobalEvent: UsersChanged re-reads the users', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('reloads the users and this page\'s reader devices', () => {
    handleGlobalEvent('UsersChanged', {});
    expect(loadGatewayUsers).toHaveBeenCalledTimes(1);
    expect(loadReaderDevices).toHaveBeenCalledTimes(1);
  });
});
