import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  activeMenuItem,
  panelOverlay,
  currentApp,
  previewFile,
  panelUrl,
} from '../store';
import type { App } from '../types';

// Mock navigation to spy on pushNavState
const pushNavState = vi.fn();
vi.mock('./navigation', () => ({ pushNavState }));

// Mock API calls triggered by switchMenuItem's data loaders
vi.mock('../../api/client', () => ({
  getNotifications: vi.fn().mockResolvedValue({ notifications: [], unread_count: 0, has_more: false }),
  listTriggers: vi.fn().mockResolvedValue({ triggers: [] }),
  listAppsApi: vi.fn().mockResolvedValue([]),
  listDevices: vi.fn().mockResolvedValue({ devices: [] }),
}));

const { switchMenuItem } = await import('./menu');

const fakeApp: App = {
  id: 'sommerferie',
  name: 'Sommerferie 2026',
  description: 'Trip planner',
  knowhow: [],
};

describe('switchMenuItem', () => {
  beforeEach(() => {
    activeMenuItem.value = 'files';
    panelOverlay.value = null;
    pushNavState.mockClear();
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
});
