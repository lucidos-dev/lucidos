import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  activeMenuItem,
  panelOverlay,
  settingsSubview,
  settingsScrollTarget,
  currentApp,
  previewFile,
  panelUrl,
  mobileView,
} from '../store';
import type { App } from '../types';
import { ladderAnchor } from '../../components/settings/intelligenceLadder';

const pushNavState = vi.fn();
vi.mock('./navigation', () => ({ pushNavState }));

const revealContentPane = vi.fn();
const navigateToPane = vi.fn();
vi.mock('./pane', () => ({ revealContentPane, navigateToPane }));

vi.mock('../../utils/viewport', () => ({ isMobile: () => true }));

vi.mock('./credentials', () => ({ loadCredentials: vi.fn().mockResolvedValue(undefined) }));

vi.mock('./environmentVariables', () => ({ loadEnvironmentVariables: vi.fn().mockResolvedValue(undefined) }));

vi.mock('../../api/client', () => ({
  getNotifications: vi.fn().mockResolvedValue({ notifications: [], unread_count: 0, has_more: false }),
  listTriggers: vi.fn().mockResolvedValue({ triggers: [] }),
  listAppsApi: vi.fn().mockResolvedValue([]),
  fetchPluginCatalog: vi.fn().mockResolvedValue({
    marketplaces: [], plugins: [], errors: [],
    scanned_at: null, scanning: false, scan_error: null,
  }),
  fetchInstalledPlugins: vi.fn().mockResolvedValue({ plugins: [] }),
  listDevices: vi.fn().mockResolvedValue({ devices: [] }),
}));

const { switchMenuItem, openSettingsSubview, setActiveMenu, landOnAccountsWithOverlay, openBackupSettings, openWebhookSettings, openCaptureContextSetting, openCacheCardsSetting, openCacheKeepAliveSetting, openIntelligenceLadder, openProviderSettings, openProviderBlock, openFreeProviderSettings } = await import('./menu');

const fakeApp: App = {
  id: 'trip-planner',
  name: 'Trip Planner 2026',
  description: 'Trip planner', reveal: 'on-load', kind: 'app', reusable: false,
};

describe('switchMenuItem', () => {
  beforeEach(() => {
    activeMenuItem.value = 'files';
    panelOverlay.value = null;
    pushNavState.mockClear();
    revealContentPane.mockClear();
    navigateToPane.mockClear();
  });

  it('clears app UI overlay when switching to a different menu item', () => {
    activeMenuItem.value = 'apps';
    panelOverlay.value = { type: 'app-ui', app: fakeApp };

    switchMenuItem('notifications');

    expect(activeMenuItem.value).toBe('notifications');
    expect(currentApp.value).toBeNull();
  });

  it('clears app UI overlay when re-selecting the SAME menu item (pinned app bug)', () => {
    activeMenuItem.value = 'notifications';
    panelOverlay.value = { type: 'app-ui', app: fakeApp };

    switchMenuItem('notifications');

    expect(activeMenuItem.value).toBe('notifications');
    expect(currentApp.value).toBeNull();
  });

  it('clears file preview when re-selecting same menu item', () => {
    activeMenuItem.value = 'files';
    panelOverlay.value = { type: 'file-preview', path: 'some/file.md' };

    switchMenuItem('files');

    expect(previewFile.value).toBeNull();
  });

  it('clears URL preview when re-selecting same menu item', () => {
    activeMenuItem.value = 'files';
    panelOverlay.value = { type: 'url-preview', url: 'https://example.com' };

    switchMenuItem('files');

    expect(panelUrl.value).toBeNull();
  });

  it('pushes navigation state on every menu switch', () => {
    switchMenuItem('notifications');
    expect(pushNavState).toHaveBeenCalledTimes(1);

    pushNavState.mockClear();
    switchMenuItem('apps');
    expect(pushNavState).toHaveBeenCalledTimes(1);

    pushNavState.mockClear();
    switchMenuItem('settings');
    expect(pushNavState).toHaveBeenCalledTimes(1);
  });

  it('reveals the content pane on every menu switch (mobile swipe, desktop expand)', () => {
    switchMenuItem('notifications');
    expect(revealContentPane).toHaveBeenCalledTimes(1);
  });

  it('navigates to the Plugins panel and reveals the content pane', () => {
    switchMenuItem('plugins');
    expect(activeMenuItem.value).toBe('plugins');
    expect(revealContentPane).toHaveBeenCalledTimes(1);
  });

  it('reveals the content pane even when re-selecting the SAME menu item', () => {
    activeMenuItem.value = 'notifications';
    switchMenuItem('notifications');
    expect(revealContentPane).toHaveBeenCalledTimes(1);
  });

});

describe('openSettingsSubview', () => {
  beforeEach(() => {
    activeMenuItem.value = 'settings';
    settingsSubview.value = 'main';
    panelOverlay.value = null;
    pushNavState.mockClear();
    revealContentPane.mockClear();
    navigateToPane.mockClear();
  });

  it('clears app UI overlay when navigating to a settings subview', () => {
    panelOverlay.value = { type: 'app-ui', app: fakeApp };

    openSettingsSubview('accounts');

    expect(settingsSubview.value).toBe('accounts');
    expect(panelOverlay.value).toBeNull();
    expect(currentApp.value).toBeNull();
  });

  it('clears file preview overlay when navigating to a settings subview', () => {
    panelOverlay.value = { type: 'file-preview', path: 'some/file.md' };

    openSettingsSubview('devices');

    expect(settingsSubview.value).toBe('devices');
    expect(previewFile.value).toBeNull();
  });

  it('pushes navigation state', () => {
    openSettingsSubview('memory');
    expect(pushNavState).toHaveBeenCalledTimes(1);
  });

  it('lands the Settings menu item itself, in a SINGLE nav push', () => {
    activeMenuItem.value = 'notifications';

    openSettingsSubview('backup');

    expect(activeMenuItem.value).toBe('settings');
    expect(settingsSubview.value).toBe('backup');
    expect(pushNavState).toHaveBeenCalledTimes(1);
  });

  it('clears an open overlay so a deep link is not covered by it', () => {
    activeMenuItem.value = 'notifications';
    panelOverlay.value = { type: 'app-ui', app: fakeApp };

    openSettingsSubview('backup');

    expect(panelOverlay.value).toBeNull();
    expect(settingsSubview.value).toBe('backup');
  });

  it('reveals the content pane (mobile swipe, desktop expand)', () => {
    openSettingsSubview('devices');
    expect(revealContentPane).toHaveBeenCalledTimes(1);
  });

  it('opens the environment-variables subview', () => {
    openSettingsSubview('environment-variables');
    expect(settingsSubview.value).toBe('environment-variables');
    expect(revealContentPane).toHaveBeenCalledTimes(1);
  });

  it('opens the thread-queue subview (Thread Queue lives under Settings → System)', () => {
    openSettingsSubview('thread-queue');
    expect(settingsSubview.value).toBe('thread-queue');
    expect(revealContentPane).toHaveBeenCalledTimes(1);
  });

  it('opens the models subview (the triggering deep-link case)', () => {
    openSettingsSubview('models');
    expect(settingsSubview.value).toBe('models');
    expect(revealContentPane).toHaveBeenCalledTimes(1);
  });

  it('opens the permissions subview', () => {
    openSettingsSubview('permissions');
    expect(settingsSubview.value).toBe('permissions');
    expect(revealContentPane).toHaveBeenCalledTimes(1);
  });
});

describe('setActiveMenu (pure plumbing)', () => {
  beforeEach(() => {
    activeMenuItem.value = 'files';
    panelOverlay.value = null;
    pushNavState.mockClear();
    revealContentPane.mockClear();
    navigateToPane.mockClear();
    mobileView.value = 'thread';
  });

  it('does NOT touch pane navigation — that is the user-intent layer job', () => {
    setActiveMenu('notifications');
    expect(revealContentPane).not.toHaveBeenCalled();
    expect(navigateToPane).not.toHaveBeenCalled();
  });
});

describe('landOnAccountsWithOverlay', () => {
  beforeEach(() => {
    activeMenuItem.value = 'files';
    settingsSubview.value = 'main';
    panelOverlay.value = null;
    pushNavState.mockClear();
    revealContentPane.mockClear();
    navigateToPane.mockClear();
  });

  it('reveals the content pane on the same render as the deep link lands', () => {
    landOnAccountsWithOverlay({ type: 'form', form: { type: 'credential' } });
    expect(activeMenuItem.value).toBe('settings');
    expect(settingsSubview.value).toBe('accounts');
    expect(revealContentPane).toHaveBeenCalledTimes(1);
  });
});

describe('openBackupSettings', () => {
  beforeEach(() => {
    activeMenuItem.value = 'files';
    settingsSubview.value = 'main';
    panelOverlay.value = null;
    pushNavState.mockClear();
    revealContentPane.mockClear();
    navigateToPane.mockClear();
  });

  it('lands on Settings > Backup and reveals the content pane', () => {
    openBackupSettings();
    expect(activeMenuItem.value).toBe('settings');
    expect(settingsSubview.value).toBe('backup');
    expect(revealContentPane).toHaveBeenCalledTimes(1);
    expect(pushNavState).toHaveBeenCalledTimes(1);
  });
});

describe('openWebhookSettings', () => {
  beforeEach(() => {
    activeMenuItem.value = 'files';
    settingsSubview.value = 'main';
    panelOverlay.value = null;
    pushNavState.mockClear();
    revealContentPane.mockClear();
    navigateToPane.mockClear();
  });

  it('lands on Settings > Webhooks and reveals the content pane', () => {
    openWebhookSettings();
    expect(activeMenuItem.value).toBe('settings');
    expect(settingsSubview.value).toBe('webhooks');
    expect(revealContentPane).toHaveBeenCalledTimes(1);
    expect(pushNavState).toHaveBeenCalledTimes(1);
  });
});

describe('Debugging row deep links', () => {
  beforeEach(() => {
    activeMenuItem.value = 'files';
    settingsSubview.value = 'main';
    settingsScrollTarget.value = null;
    panelOverlay.value = null;
    pushNavState.mockClear();
    revealContentPane.mockClear();
  });

  it('lands on the Capture context row in Settings > Debugging', () => {
    openCaptureContextSetting();
    expect(activeMenuItem.value).toBe('settings');
    expect(settingsSubview.value).toBe('debugging');
    expect(settingsScrollTarget.value).toBe('debugging:capture-context');
    expect(revealContentPane).toHaveBeenCalledTimes(1);
    expect(pushNavState).toHaveBeenCalledTimes(1);
  });

  it('lands on the Cache cards row under Settings > Models > Cache keep-alive', () => {
    openCacheCardsSetting();
    expect(activeMenuItem.value).toBe('settings');
    expect(settingsSubview.value).toBe('models');
    expect(settingsScrollTarget.value).toBe('models:cache-cards');
    expect(revealContentPane).toHaveBeenCalledTimes(1);
    expect(pushNavState).toHaveBeenCalledTimes(1);
  });
});

describe('Coding Agents row deep links', () => {
  beforeEach(() => {
    activeMenuItem.value = 'files';
    settingsSubview.value = 'main';
    settingsScrollTarget.value = null;
    panelOverlay.value = null;
    pushNavState.mockClear();
    revealContentPane.mockClear();
  });

  it('lands on the Cache keep-alive section in Settings > Models', () => {
    openCacheKeepAliveSetting();
    expect(activeMenuItem.value).toBe('settings');
    expect(settingsSubview.value).toBe('models');
    expect(settingsScrollTarget.value).toBe('models:cache-keep-alive');
    expect(revealContentPane).toHaveBeenCalledTimes(1);
    expect(pushNavState).toHaveBeenCalledTimes(1);
  });
});

describe('Providers page deep links', () => {
  beforeEach(() => {
    activeMenuItem.value = 'files';
    settingsSubview.value = 'main';
    settingsScrollTarget.value = null;
    panelOverlay.value = null;
    pushNavState.mockClear();
    revealContentPane.mockClear();
  });

  it('opens the Providers page for first-run onboarding', () => {
    openProviderSettings();
    expect(activeMenuItem.value).toBe('settings');
    expect(settingsSubview.value).toBe('providers');
    expect(revealContentPane).toHaveBeenCalledTimes(1);
    expect(pushNavState).toHaveBeenCalledTimes(1);
  });

  it('opens one provider block, scrolled to', () => {
    openProviderBlock('xai');
    expect(settingsSubview.value).toBe('providers');
    expect(settingsScrollTarget.value).toBe('providers:xai');
    expect(revealContentPane).toHaveBeenCalledTimes(1);
  });

  it('lands the keyless shortcut on the OpenCode Free switch', () => {
    openFreeProviderSettings();
    expect(settingsSubview.value).toBe('providers');
    expect(settingsScrollTarget.value).toBe('providers:opencode-free');
  });
});

describe('openIntelligenceLadder', () => {
  beforeEach(() => {
    activeMenuItem.value = 'files';
    settingsSubview.value = 'main';
    settingsScrollTarget.value = null;
    panelOverlay.value = null;
    pushNavState.mockClear();
    revealContentPane.mockClear();
  });

  it('lands on one surface ladder in Settings > Intelligence', () => {
    openIntelligenceLadder('codex');
    expect(activeMenuItem.value).toBe('settings');
    expect(settingsSubview.value).toBe('intelligence');
    expect(settingsScrollTarget.value).toBe(ladderAnchor('codex'));
    expect(revealContentPane).toHaveBeenCalledTimes(1);
    expect(pushNavState).toHaveBeenCalledTimes(1);
  });
});
