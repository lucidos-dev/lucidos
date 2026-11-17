// Authoritative native-window *active* state for the Tauri desktop client.
//
// The embedded WKWebView can't observe macOS `orderOut:` — a window dismissed to
// the menu-bar tray keeps `document.visibilityState === 'visible'` and
// `document.hasFocus() === true` (macOS posts no occlusion change for orderOut:)
// — and its `hasFocus()` is unreliable generally, so the page can't tell "in
// use" from "trayed / behind another app" on its own. The Rust side emits a
// `native-window-active` event (a bare boolean: focused AND on-screen) from the
// authoritative AppKit focus + hide/show state; we cache it and feed it into
// isPageActive() so a non-active desktop client gets the OS native banner instead
// of a suppressed, invisible in-app toast.
//
// Always `true` in the browser / PWA — no event ever arrives, so the default
// leaves web behavior unchanged. See system-knowhow/notifications.md §1, §4.

import { isTauri } from './platform';
import { getNativeWindowActive, getNativeWindowShown, listen } from './tauri';

let nativeWindowActive = true;

/** Whether the native (Tauri) window is currently active — focused and on-screen.
 *  Always `true` off-Tauri (no native bridge). Consulted by isPageActive(). */
export function isNativeWindowActive(): boolean {
  return nativeWindowActive;
}

/** Set the cached native-window active state. Called by the listen callback and
 *  by tests. Pure — only mutates the module cache. */
export function setNativeWindowActive(active: boolean): void {
  nativeWindowActive = active;
}

/** Subscribe to native-window active changes (Tauri only). Updates the cache
 *  BEFORE invoking `onChange`, so a handler that reads isNativeWindowActive() /
 *  isPageActive() sees the new value. Returns an unlisten; a no-op off-Tauri.
 *
 *  SEEDS the cache from the authoritative AppKit state (the Rust
 *  `get_native_window_active` command) BEFORE registering the listener. This is
 *  load-bearing: Tauri does not replay the `native-window-active` transition
 *  events (emitted from on_window_event) to a listener that registers after the
 *  fact, and the cache defaults to `true` — so without the seed a freshly
 *  (re)loaded page that is actually backgrounded/trayed (crash-watchdog reload,
 *  version refresh, cold/unfocused launch) keeps reporting the device active,
 *  and the engine suppresses the OS push into an invisible in-app toast (no
 *  banner). See system-knowhow/notifications.md §3/§4. */
export async function startNativeWindowActiveTracking(
  onChange?: (active: boolean) => void,
): Promise<() => void> {
  if (!isTauri()) return () => {};
  try {
    const active = await getNativeWindowActive();
    setNativeWindowActive(active);
    onChange?.(active);
  } catch {
    // Best-effort seed (telemetry carve-out, .claude/rules/frontend.md): a
    // failed seed leaves the cache at its default; the next focus transition /
    // heartbeat corrects it. No user intent here.
  }
  return listen<boolean>('native-window-active', (e) => {
    setNativeWindowActive(e.payload);
    onChange?.(e.payload);
  });
}

export type NativeWindowShownTracking = {
  status: 'supported' | 'unsupported';
  stop(): void;
};

/** Track native hide/show intent. Listen before seeding so transitions win.
 * Older shells lack this protocol; only its exact capability errors opt out. */
export async function startNativeWindowShownTracking(
  onChange: (shown: boolean) => void,
  signal?: AbortSignal,
): Promise<NativeWindowShownTracking> {
  if (!isTauri()) return { status: 'unsupported', stop() {} };
  let transitioned = false;
  let stopped = false;
  const unlisten = await listen<boolean>('native-window-shown', (event) => {
    if (stopped || signal?.aborted) return;
    transitioned = true;
    onChange(event.payload);
  });
  const stop = () => {
    if (stopped) return;
    stopped = true;
    signal?.removeEventListener('abort', stop);
    unlisten();
  };
  signal?.addEventListener('abort', stop, { once: true });
  try {
    if (signal?.aborted) throw new Error('Native window visibility tracking stopped.');
    const shown = await getNativeWindowShown();
    if (signal?.aborted) throw new Error('Native window visibility tracking stopped.');
    if (!transitioned) onChange(shown);
    return { status: 'supported', stop };
  } catch (error) {
    stop();
    const message = error instanceof Error ? error.message : error;
    if (message === 'Command get_native_window_shown not allowed by ACL'
      || message === 'Command get_native_window_shown not found') {
      return { status: 'unsupported', stop };
    }
    throw error;
  }
}
