import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  focusedThreadId,
  panelOverlay,
  preferences,
  threadMap,
  threadsLoaded,
  toasts,
  wipPreviewThreadId,
} from '../store';
import type { App } from '../types';
import { makeThreadState } from './threads-test-helpers';

const openExternal = vi.hoisted(() => vi.fn(() => Promise.resolve()));
vi.mock('../../utils/tauri', () => ({
  openExternal,
  setTitlebarColor: () => Promise.resolve(),
  windowReadyToShow: () => {},
}));

vi.mock('./navigation', () => ({ pushNavState: vi.fn() }));
vi.mock('./pane', () => ({ revealContentPane: vi.fn() }));

vi.mock('../../api/client', () => ({
  appUrl: (id: string, threadId?: string) =>
    threadId ? `/dev/app/${id}/?thread_id=${threadId}` : `/dev/app/${id}/`,
  listAppsApi: vi.fn().mockResolvedValue([]),
  postAppCapture: vi.fn().mockResolvedValue(undefined),
}));

const route = vi.hoisted(() => ({ basePath: '/dev', gateway: null as string | null }));
vi.mock('../../utils/basePath', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/basePath')>()),
  get BASE_PATH() { return route.basePath; },
  get REMOTE_GATEWAY() { return route.gateway; },
}));

const { appPopoutHref, popOutApp } = await import('./apps');

const habitTracker: App = {
  id: 'habit-tracker',
  name: 'Habit Tracker',
  description: 'Daily habits', reveal: 'on-load', kind: 'app', reusable: false,
};

const PAGE_URL = 'http://localhost:4711/dev/';

describe('popOutApp hands the open app to the OS opener', () => {
  beforeEach(() => {
    panelOverlay.value = { type: 'app-ui', app: habitTracker };
    wipPreviewThreadId.value = null;
    threadMap.value = new Map();
    threadsLoaded.value = false;
    focusedThreadId.value = null;
    preferences.value = { status: 'loaded', data: {} };
    toasts.value = [];
    openExternal.mockClear();
    openExternal.mockResolvedValue(undefined);
    vi.stubGlobal('location', { href: PAGE_URL });
    route.basePath = '/dev';
    route.gateway = null;
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('opens an ABSOLUTE url, so the gateway origin, port and slug survive', () => {
    popOutApp();

    expect(openExternal).toHaveBeenCalledWith('http://localhost:4711/dev/app/habit-tracker/');
  });

  it('carries the WIP preview thread through, so a popped-out preview is still the preview', () => {
    threadMap.value = new Map([['thread-7', makeThreadState('thread-7')]]);
    threadsLoaded.value = true;
    focusedThreadId.value = 'thread-7';
    wipPreviewThreadId.value = 'thread-7';

    popOutApp();

    expect(openExternal).toHaveBeenCalledWith(
      'http://localhost:4711/dev/app/habit-tracker/?thread_id=thread-7',
    );
  });

  it('never mounts the in-app url-preview panel, even with that preference ON', () => {
    preferences.value = { status: 'loaded', data: { experimental_in_app_browser: 'true' } };

    popOutApp();

    expect(panelOverlay.value).toEqual({ type: 'app-ui', app: habitTracker });
    expect(openExternal).toHaveBeenCalledTimes(1);
  });

  it('says so when there is no app open, instead of opening the shell itself', () => {
    panelOverlay.value = null;

    popOutApp();

    expect(openExternal).not.toHaveBeenCalled();
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].type).toBe('error');
  });

  it('surfaces an opener failure with the url in it', async () => {
    openExternal.mockRejectedValue(new Error('no handler for scheme'));

    popOutApp();
    await vi.waitFor(() => expect(toasts.value).toHaveLength(1));

    expect(toasts.value[0].type).toBe('error');
    expect(toasts.value[0].message).toContain('http://localhost:4711/dev/app/habit-tracker/');
    expect(toasts.value[0].message).toContain('no handler for scheme');
  });

  it('a remote workspace opens its own shell with an #app= landing, never the bare app', () => {
    route.basePath = '/dev@office';
    route.gateway = 'office';
    vi.stubGlobal('location', { href: 'http://localhost:4711/dev@office/' });

    popOutApp();

    expect(openExternal).toHaveBeenCalledWith('http://localhost:4711/dev@office/#app=habit-tracker');
  });
});

describe('appPopoutHref, which the browser anchor reads', () => {
  beforeEach(() => {
    panelOverlay.value = { type: 'app-ui', app: habitTracker };
    wipPreviewThreadId.value = null;
    route.basePath = '/dev';
    route.gateway = null;
  });

  it('local: the bare app URL', () => {
    expect(appPopoutHref()).toBe('/dev/app/habit-tracker/');
  });

  it('remote: the shell landing, carrying the app fragment', () => {
    route.basePath = '/dev@office';
    route.gateway = 'office';
    panelOverlay.value = { type: 'app-ui', app: habitTracker, fragment: 'pr-1645' };

    const href = appPopoutHref();

    expect(href).toBe('/dev@office/#app=habit-tracker&fragment=pr-1645');
    expect(href).not.toContain('/app/');
  });

  it('null when no app is open', () => {
    panelOverlay.value = null;
    route.gateway = 'office';
    expect(appPopoutHref()).toBeNull();
  });
});
