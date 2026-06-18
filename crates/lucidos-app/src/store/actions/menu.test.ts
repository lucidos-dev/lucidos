import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  activeMenuItem,
  panelOverlay,
  settingsSubview,
  currentApp,
  previewFile,
  panelUrl,
  mobileView,
} from '../store';
import type { App } from '../types';

// Mock navigation to spy on pushNavState
const pushNavState = vi.fn();
vi.mock('./navigation', () => ({ pushNavState }));

// Mock pane helper — single rule across the codebase: any user-intent navigation
// that lands content into the right pane must call revealContentPane(), so the
// mobile user gets swiped to it AND the desktop user's collapsed split expands.
// We mock here to verify each helper in this file calls it (or, for the pure
// plumbing helper setActiveMenu, that it does NOT).
const revealContentPane = vi.fn();
const navigateToPane = vi.fn();
vi.mock('./pane', () => ({ revealContentPane, navigateToPane }));

// Force isMobile() to return true so the setActiveMenu pure-plumbing pin would
// have ALSO failed under the pre-refactor conditional (`item !== prev &&
// isMobile() && mobileView === 'thread'`). jsdom's default viewport is desktop;
// without this mock the conditional would skip on isMobile() alone and the
// pin would pass for the wrong reason — green even before the refactor.
vi.mock('../../utils/viewport', () => ({ isMobile: () => true }));

// Mock credentials loader (called by openSettingsSubview('accounts'))
vi.mock('./credentials', () => ({ loadCredentials: vi.fn().mockResolvedValue(undefined) }));

// Mock env var loader (called by openSettingsSubview('environment-variables'))
vi.mock('./environmentVariables', () => ({ loadEnvironmentVariables: vi.fn().mockResolvedValue(undefined) }));

// Mock API calls triggered by switchMenuItem's data loaders
vi.mock('../../api/client', () => ({
  getNotifications: vi.fn().mockResolvedValue({ notifications: [], unread_count: 0, has_more: false }),
  listTriggers: vi.fn().mockResolvedValue({ triggers: [] }),
  listAppsApi: vi.fn().mockResolvedValue([]),
  fetchPluginCatalog: vi.fn().mockResolvedValue({ marketplaces: [], plugins: [], errors: [] }),
  listDevices: vi.fn().mockResolvedValue({ devices: [] }),
}));

const { switchMenuItem, openSettingsSubview, setActiveMenu, landOnAccountsWithOverlay } = await import('./menu');

const fakeApp: App = {
  id: 'trip-planner',
  name: 'Trip Planner 2026',
  description: 'Trip planner',
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
    // BUG SCENARIO:
    // 1. activeMenuItem is 'notifications' (saved from previous visit)
    // 2. User opened pinned app UI (doesn't change activeMenuItem)
    // 3. User clicks notification bell → switchMenuItem('notifications')
    // 4. item === prev, so clearing was skipped → app UI stayed visible
    activeMenuItem.value = 'notifications';
    panelOverlay.value = { type: 'app-ui', app: fakeApp };

    switchMenuItem('notifications');

    expect(activeMenuItem.value).toBe('notifications');
    // These must be cleared even though item === prev
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
    // The user-intent rule: any helper that puts something into the right-hand
    // content pane must call revealContentPane(). switchMenuItem is the
    // user-intent layer for drawer clicks, NotificationsBell, nav-link clicks,
    // and SDK navigate_ui → handleNavigationRequest panel branches.
    switchMenuItem('notifications');
    expect(revealContentPane).toHaveBeenCalledTimes(1);
  });

  it('reveals the content pane even when re-selecting the SAME menu item', () => {
    // Regression for the old setActiveMenu gate that skipped pane navigation
    // when `item === prev`. Re-tapping a link to a panel the user previously
    // visited (e.g. activeMenuItem stuck at 'notifications' from prior visit)
    // while looking at a thread must STILL swipe to content.
    activeMenuItem.value = 'notifications';
    switchMenuItem('notifications');
    expect(revealContentPane).toHaveBeenCalledTimes(1);
  });

  it('navigates to the Thread Queue panel like any other menu item', () => {
    // Mirror entry for the Thread Queue navigation point (frontend.md: new
    // navigation entry points get pinned here). The panel rides the standard
    // switchMenuItem path — active item set + content pane revealed.
    switchMenuItem('thread-queue');
    expect(activeMenuItem.value).toBe('thread-queue');
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

  it('reveals the content pane (mobile swipe, desktop expand)', () => {
    // Settings sub-sections live inside the content pane. Opening one from
    // any source (deep link, search result, in-panel nav) must swipe to it
    // on mobile — without this, tapping a settings deep-link from a chat on
    // mobile silently left the user on the thread pane.
    openSettingsSubview('devices');
    expect(revealContentPane).toHaveBeenCalledTimes(1);
  });

  it('opens the environment-variables subview', () => {
    openSettingsSubview('environment-variables');
    expect(settingsSubview.value).toBe('environment-variables');
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
    // Force the conditions under which the OLD setActiveMenu's
    // `if (item !== prev && isMobile() && mobileView === 'thread')` block
    // WOULD have fired (item changes from 'files' to 'notifications', isMobile
    // is forced true, mobileView is the default 'thread'). This makes the
    // pin a real regression guard — if someone re-introduces the conditional,
    // the test breaks; the test cannot pass for the wrong reason because the
    // pre-refactor branch would have called navigateToPane here.
    mobileView.value = 'thread';
  });

  it('does NOT touch pane navigation — that is the user-intent layer job', () => {
    // Pin: setActiveMenu is internal plumbing used by multiple flows
    // (switchMenuItem, landOnAccountsWithOverlay, thread-sync new-app /
    // new-trigger branches). Each user-intent caller is responsible for
    // calling revealContentPane() itself. Putting pane logic inside
    // setActiveMenu — gated on `item !== prev && mobileView === 'thread'` —
    // was the original bug: silent no-swipe when the user re-tapped the same
    // item, or was already on the threads pane.
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
