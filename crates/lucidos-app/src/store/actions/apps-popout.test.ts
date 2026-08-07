/**
 * The app popout, on the one client where an anchor cannot do it.
 *
 * In a browser the control is a real `<a target="_blank">` and needs no action
 * at all. In the packaged desktop client that anchor is a dead click: the href
 * is root-relative so `onGlobalClick`'s `^https?://` funnel never claims it, and
 * WKWebView then drops the `_blank` navigation because wry installs a
 * new-window delegate only on the in-app browser preview webview, never on the
 * `main` window. `popOutApp` is what stands in there, and the two ways it can go
 * wrong are both invisible from the call site: handing the OS a root-relative
 * path (macOS `open` cannot resolve one), and routing through `openUrl`, which
 * with the experimental in-app browser preference on would mount the
 * url-preview panel INSIDE the shell, over the very app being popped out of it.
 */
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

// The OS opener (Tauri `open_url_external`, which runs macOS `open`).
// setTitlebarColor is unused here, but the real preferences module sits in
// apps.ts's import graph and pulls it from this module, so the mock must
// provide it.
const openExternal = vi.hoisted(() => vi.fn(() => Promise.resolve()));
vi.mock('../../utils/tauri', () => ({
  openExternal,
  setTitlebarColor: () => Promise.resolve(),
}));

vi.mock('./navigation', () => ({ pushNavState: vi.fn() }));
vi.mock('./pane', () => ({ revealContentPane: vi.fn() }));

vi.mock('../../api/client', () => ({
  appUrl: (id: string, threadId?: string) =>
    threadId ? `/dev/app/${id}/?thread_id=${threadId}` : `/dev/app/${id}/`,
  listAppsApi: vi.fn().mockResolvedValue([]),
  postAppCapture: vi.fn().mockResolvedValue(undefined),
}));

const { popOutApp } = await import('./apps');

const habitTracker: App = {
  id: 'habit-tracker',
  name: 'Habit Tracker',
  description: 'Daily habits',
};

/** The page the packaged client is on: the gateway origin, under the workspace
 *  slug. The popout URL has to come out on this origin and port. */
const PAGE_URL = 'http://localhost:4711/dev/';

describe('popOutApp hands the open app to the OS opener', () => {
  beforeEach(() => {
    // `currentApp` is a computed over `panelOverlay`, so the app-ui overlay IS
    // "an app is open" and there is no second signal to fall out of step.
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
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('opens an ABSOLUTE url, so the gateway origin, port and slug survive', () => {
    popOutApp();

    // Not `/dev/app/habit-tracker/`: macOS `open` cannot resolve a path, and the
    // engine is not on the default port.
    expect(openExternal).toHaveBeenCalledWith('http://localhost:4711/dev/app/habit-tracker/');
  });

  it('carries the WIP preview thread through, so a popped-out preview is still the preview', () => {
    // The WIP auto-revert effect (actions/wipPreview, installed by apps.ts's
    // import of it) clears the preview unless the WIP thread is the focused one
    // and is in the map, so seed both rather than poking the signal alone.
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
    // The whole reason this does not route through `openUrl`. That panel lives
    // inside the shell, so "pop out" would have replaced the app with a webview
    // of the same app, in the pane it was already in.
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
});
