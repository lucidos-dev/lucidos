import { recordIpcOutcome } from './ipcHealth';

declare global {
  interface Window {
    __TAURI_INTERNALS__?: {
      invoke: <T>(cmd: string, args?: Record<string, unknown>, options?: unknown) => Promise<T>;
      transformCallback: (callback: (response: unknown) => void, once?: boolean) => number;
    };
    /** Injected by the Tauri app on page load — CalVer version at build time. */
    __LUCIDOS_APP_VERSION__?: string;
  }
}

/** Invoke a Tauri command via IPC. Only call when isTauri() is true.
 *
 *  Every command goes through here, so this is where the health of the bridge
 *  itself is observed: outcomes feed `recordIpcOutcome`, which writes a durable
 *  `[Client/ipc]` line to the engine log when calls start failing and another
 *  when they recover. Individual call sites are free to keep swallowing their own
 *  rejection (`.catch(() => {})` on the heartbeat, `console.warn` in the
 *  native-push handlers) — that no longer costs us the signal, which is what let
 *  a total ACL-driven bridge failure run silently for a month. See
 *  utils/ipcHealth. The returned promise is untouched: reporting must not change
 *  what callers see. */
export function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return window.__TAURI_INTERNALS__!.invoke<T>(cmd, args).then(
    (value) => {
      recordIpcOutcome(cmd);
      return value;
    },
    (error: unknown) => {
      recordIpcOutcome(cmd, error ?? new Error('rejected with no error'));
      throw error;
    },
  );
}

/**
 * Ref-counted panel webview visibility. Multiple overlays can request hide
 * independently; the webview only shows when all have released.
 */
let hideCount = 0;

export function hidePanelWebview(): void {
  hideCount++;
  if (hideCount === 1) {
    invoke('hide_panel_webview').catch(() => {});
  }
}

export function showPanelWebview(): void {
  hideCount = Math.max(0, hideCount - 1);
  if (hideCount === 0) {
    invoke('show_panel_webview').catch(() => {});
  }
}

export function webviewGoBack(): void {
  invoke('webview_go_back').catch(() => {});
}

export function webviewGoForward(): void {
  invoke('webview_go_forward').catch(() => {});
}

export function webviewReload(url: string): void {
  invoke('navigate_panel_webview', { url }).catch(() => {});
}

/** Extract the text content and title from the panel webview. Only call when isTauri() is true. */
export async function getWebviewContent(): Promise<{ title: string; content: string }> {
  return invoke<{ title: string; content: string }>('webview_get_content');
}

/**
 * Tint the macOS window background to match the in-app header. Under
 * `titleBarStyle: "Overlay"` the webview paints the reclaimed title-bar band (the
 * `.titlebar-strip`); this colors the behind-the-webview fallback so that band
 * reads blue, not black, before the page paints. `color` is a CSS hex string (the
 * header-gradient top stop for the active theme). Sets the window layer only, so
 * the page background isn't tinted. Only call when isTauri() is true. */
export function setTitlebarColor(color: string): Promise<void> {
  return invoke('set_titlebar_color', { color });
}

/**
 * Start a native drag of the calling window. Used by `useWindowDragRegion` once
 * the pointer crosses the drag threshold over a non-interactive area of the
 * title-bar band. App command (always allowed) — replaces the ACL-blocked
 * `data-tauri-drag-region`. Best-effort; only call when isTauri() is true. */
export function startWindowDrag(): Promise<void> {
  return invoke('start_window_drag');
}

/**
 * Toggle the calling window between maximized (macOS zoom) and restored. Bound to
 * a double-click on the reclaimed title-bar strip only. Only call when isTauri()
 * is true. */
export function toggleWindowMaximize(): Promise<void> {
  return invoke('toggle_window_maximize');
}

/**
 * Show a native macOS notification banner via the app's own
 * `show_native_notification` command (notifications.rs). We drive Apple's modern
 * `UserNotifications` framework (`UNUserNotificationCenter`) in Rust — not
 * `tauri-plugin-notification` / `mac-notification-sys`, which sit on the
 * deprecated `NSUserNotification` API that no longer delivers on recent macOS —
 * and capture the tap via a delegate. `deepLink` is the SW-message shape
 * (`notification_id` /
 * `thread_id` / `event_id` / `tap`); on tap the command emits
 * `native-notification-tapped` with it, which the page routes through the same
 * dispatchDeepLink the web-push tap uses. Only call when isTauri() is true.
 */
export async function showNativeNotification(opts: {
  title: string;
  body: string;
  deepLink: Record<string, unknown>;
}): Promise<void> {
  // `link` (single word) matches the Rust command param name verbatim.
  await invoke('show_native_notification', {
    title: opts.title,
    body: opts.body,
    link: opts.deepLink,
  });
}

/**
 * Remove an already-delivered native macOS banner via the app's
 * `dismiss_native_notification` command (notifications.rs →
 * `UNUserNotificationCenter.removeDeliveredNotifications(withIdentifiers:)` /
 * `removeAllDeliveredNotifications`). The cross-device dismiss counterpart of
 * `showNativeNotification`: when a notification is read elsewhere, the engine
 * broadcasts `NativePushDismissRequested` and the connected desktop app removes
 * its banner. `notificationId === null` removes ALL delivered banners (the
 * mark-all-read path). No-op in dev / off macOS (Rust side). Only call when
 * isTauri() is true.
 */
export async function dismissNativeNotification(opts: {
  notificationId: string | null;
}): Promise<void> {
  // `id` matches the Rust command param name (`Option<String>`); null → dismiss all.
  await invoke('dismiss_native_notification', { id: opts.notificationId });
}

/**
 * Bring the CALLING page's own native window to the front (Rust
 * `focus_calling_window`): leave menu-bar-only, unminimize, show, focus,
 * activate frontmost.
 *
 * For the one flow that finishes OUTSIDE the app: an OAuth authorization the
 * user completes in a browser, after which they'd otherwise be left on the
 * callback tab with Lucidos behind it.
 *
 * The window it fronts is the calling page's, never `main`. The command used to
 * be `focus_main_window` and reshowed `main` specifically (creating one if it
 * was gone), which raised the wrong window whenever the flow was finished from a
 * New Window or a second workspace. Not a general "focus me" the page may call
 * freely: a window that fronts itself on a background event is a nuisance, so
 * keep callers to "this page started it, the user clicked seconds ago, fires
 * once". Best-effort: errors are swallowed (there is no window to front on a
 * failed IPC, and the connection itself already succeeded). Only call when
 * isTauri() is true. */
export function focusCallingWindow(): void {
  invoke('focus_calling_window').catch(() => {});
}

/**
 * Wake the native unread-indicator loop for an immediate recompute (Rust
 * `nudge_dock_badge` command, whose name predates the tray surface). The page
 * calls this from its notification SSE handler so the macOS count updates the
 * instant a notification is read, in-app or from another device, instead of
 * waiting for the desktop poll. The recompute writes the menu-bar tray title
 * always, and the dock badge as well while a client window is open; it reads the
 * gateway's fresh `unread-total` aggregate, so this carries no count.
 * Best-effort: errors are swallowed (neither surface exists in dev / non-macOS).
 * Only call when isTauri() is true. */
export function nudgeDockBadge(): void {
  invoke('nudge_dock_badge').catch(() => {});
}

/**
 * Report whether the native main window is currently *active* — focused AND
 * on-screen (visible, not minimized) — read live from AppKit by the Rust
 * `get_native_window_active` command. The page pulls this at startup to SEED its
 * `native-window-active` cache before registering the event listener, because
 * Tauri doesn't replay the transition events to a late-registering listener and
 * the cache defaults to `true` (see utils/nativeWindow.ts). Only call when
 * isTauri() is true. */
export function getNativeWindowActive(): Promise<boolean> {
  return invoke<boolean>('get_native_window_active');
}

/**
 * Drain (and clear) the deep links from native-banner taps the page may not
 * have been listening for at emit time (webview reloaded / suspended-while-
 * trayed / client relaunched). Returned in SW-message shape so each routes
 * through the same dispatchDeepLink as a live tap / web-push tap. The drain is
 * atomic in Rust, so calling it from both the startup cold path and the
 * `native-notification-tapped` warm signal routes each tap exactly once. Only
 * call when isTauri() is true. */
export function takePendingNativeTaps(): Promise<Record<string, unknown>[]> {
  return invoke<Record<string, unknown>[]>('take_pending_native_taps');
}

/**
 * Durable get-or-create of this device's id for `workspace` (its gateway slug),
 * backed by a native JSON map in the App Support data dir that survives a DMG
 * reinstall (unlike the WKWebView's `localStorage`, which a new bundle re-buckets).
 * Returns the stored id when one exists, else stores and returns `candidate`. The
 * caller seeds the result back into `localStorage` so the synchronous
 * `getDeviceId()` is unchanged. Only call when isTauri() is true. */
export function getOrCreateDeviceId(workspace: string, candidate: string): Promise<string> {
  return invoke<string>('get_or_create_device_id', { workspace, candidate });
}

// --- App auto-update (packaged desktop app) ---

/** Check GitHub Releases for a newer signed packaged build. Returns the new
 *  version string when an update is available, else null. Drives the in-app
 *  update toast. Only call when isTauri() is true (no-op → null in dev). */
export function checkAppUpdate(): Promise<string | null> {
  return invoke<string | null>('check_app_update');
}

/** Install the available packaged update and restart the WHOLE stack — the
 *  launchd background service (gateway + engines + embedded Postgres) AND the GUI
 *  client — onto the new version. On success the client re-execs and this promise
 *  never resolves; it rejects with a string error otherwise (no update, download
 *  failure). Progress arrives out-of-band on {@link APP_UPDATE_PROGRESS_EVENT} —
 *  this promise says nothing until it is over. Only call when isTauri() is true. */
export function installAppUpdateAndRestart(): Promise<void> {
  return invoke('install_app_update_and_restart');
}

/** Tauri event carrying {@link AppUpdateProgress} frames while an update runs.
 *  Emitted by `src/updater.rs`; the name must match `PROGRESS_EVENT` there. */
export const APP_UPDATE_PROGRESS_EVENT = 'app-update-progress';

/** Where a packaged update run currently is — the TypeScript mirror of Rust's
 *  `AppUpdatePhase` (`src/updater.rs`), serialized internally-tagged on `phase`.
 *
 *  A discriminated union rather than a bare string so the phase's data travels
 *  with it (only `downloading` has byte counts, only `failed` has a message) and
 *  `tsc` forces every consumer to handle every phase. `total` is null when the
 *  server declared no `Content-Length`: there is then no honest percentage, and
 *  the UI must show bytes alone rather than invent one. */
interface AppUpdateFrame {
  /** The version being installed; null until the check resolves one. */
  version: string | null;
}

export type AppUpdateProgress =
  | (AppUpdateFrame & { phase: 'checking' })
  | (AppUpdateFrame & { phase: 'downloading'; downloaded: number; total: number | null })
  | (AppUpdateFrame & { phase: 'verifying' })
  | (AppUpdateFrame & { phase: 'installing' })
  | (AppUpdateFrame & { phase: 'restarting-services' })
  | (AppUpdateFrame & { phase: 'relaunching' })
  | (AppUpdateFrame & { phase: 'cancelled' })
  | (AppUpdateFrame & { phase: 'failed'; message: string })
  /** The install left no runnable app behind: the swap destroyed the old bundle
   *  without landing the new one. Its own phase rather than a longer `failed`
   *  message, because the two need different handling and not just different
   *  wording: `failed` is retryable and this is not, the recovery is a reinstall
   *  from the .dmg, and the page must not re-offer the update. Rust's
   *  `AppUpdatePhase::BundleSwapFailed`, raised from both install outcomes. */
  | (AppUpdateFrame & { phase: 'bundle-swap-failed'; message: string });

/** A frame describing a run still IN FLIGHT. `cancelled`, `failed` and
 *  `bundle-swap-failed` END a run rather than describe one, so surfaces replace
 *  themselves on those instead of updating, which is why the narration helper
 *  takes only these. */
export type AppUpdateRunning = Exclude<
  AppUpdateProgress,
  { phase: 'cancelled' | 'failed' | 'bundle-swap-failed' }
>;

/** Abandon an in-flight packaged-update download. Only the check + download can
 *  be cancelled — once the bundle swap has started the run has committed and this
 *  is a no-op. The outcome arrives as a `cancelled` progress frame, not as this
 *  promise's resolution. Only call when isTauri() is true. */
export function cancelAppUpdate(): Promise<void> {
  return invoke('cancel_app_update');
}

// --- Mobile access (packaged desktop app; macOS) ---

/** This Mac's Tailscale setup, as two INDEPENDENT facts (mirror of the Rust
 *  `TailscaleInfo`).
 *
 *  Tailnet state (`on_tailnet` / `tailnet_ip` / `magic_dns_name` / `serve_url`)
 *  is read from the machine itself with no CLI. `cli_available` gates the action
 *  buttons and nothing else: a Mac whose Tailscale works but has no CLI still
 *  gets described accurately, which before the split rendered as a Sign in
 *  button that could not work. */
export interface TailscaleInfo {
  /** Tailscale is present at all (app bundle or CLI). Drives "Get Tailscale",
   *  so it deliberately does NOT mean "usable". */
  installed: boolean;
  /** This Mac holds a tailnet address: signed in and connected. */
  on_tailnet: boolean;
  /** The tailnet IPv4. Reachable over plain HTTP from the same tailnet. */
  tailnet_ip: string | null;
  /** MagicDNS name, no scheme. Null on a tailnet with MagicDNS disabled, which
   *  is NOT the same as being offline. */
  magic_dns_name: string | null;
  /** `https://<magic_dns_name>`, set only once something is PROVEN to be
   *  serving it. Before `tailscale serve` runs, nothing listens on 443. */
  serve_url: string | null;
  /** A working `tailscale` CLI was found. Actions only, never reporting. */
  cli_available: boolean;
}

/** localhost / LAN / Tailscale connect URLs for the engine (mirror of the Rust
 *  `ConnectInfo`). The LAN URL is derived client-side from `lan_ip` + `port` +
 *  the gateway bind (`getNetworkConfig().gateway_bind`) — see
 *  `MobileAccessPage.tsx::lanRowState`. */
export interface ConnectInfo {
  port: number;
  localhost_url: string;
  lan_ip: string | null;
  tailscale: TailscaleInfo;
}

/** Open a URL in the system default browser (not the embedded webview).
 *
 *  Rejects when the OS launcher could not be STARTED (`open` / `xdg-open` /
 *  `rundll32` missing or unspawnable), so callers owe the user a toast. It does
 *  NOT reject for a launcher that starts and then fails, e.g. no application
 *  registered for the scheme: the child is fire-and-forget on the Rust side, for
 *  the reason spelled out on `open_in_default_browser` in `src/lib.rs`.
 *
 *  Only call when isTauri() is true. */
export function openExternal(url: string): Promise<void> {
  return invoke('open_url_external', { url });
}

/** Surface the engine's connect URLs (localhost / LAN / Tailscale). Only call
 *  when isTauri() is true. */
export function getConnectInfo(): Promise<ConnectInfo> {
  return invoke<ConnectInfo>('get_connect_info');
}

/** Bring the Mac onto a tailnet (`tailscale up`; interactive login). Rejects
 *  with a string error. Only call when isTauri() is true. */
export function tailscaleUp(authKey?: string): Promise<void> {
  return invoke('tailscale_up', { authKey: authKey ?? null });
}

/** Expose the engine over the tailnet (`tailscale serve`), returning the
 *  `…ts.net` URL. Rejects with a string error. Only call when isTauri() is true.
 *
 *  Resolves only when the whole run is OVER, and a run legitimately waits minutes
 *  for a tailnet approval. Everything the user sees in between arrives on
 *  {@link TAILSCALE_SERVE_PROGRESS_EVENT}. Awaiting it silently is exactly what
 *  made the button look dead. */
export function tailscaleServe(): Promise<string> {
  return invoke<string>('tailscale_serve');
}

/** Abandon the in-flight Expose run. The outcome arrives as a `cancelled` frame,
 *  not as this promise's resolution. A no-op when nothing is running. Only call
 *  when isTauri() is true. */
export function cancelTailscaleServe(): Promise<void> {
  return invoke('cancel_tailscale_serve');
}

/** Tauri event carrying {@link TailscaleServeProgress} frames while an Expose run
 *  is in flight. Emitted by `src/mobile.rs`; the name must match
 *  `SERVE_PROGRESS_EVENT` there. */
export const TAILSCALE_SERVE_PROGRESS_EVENT = 'tailscale-serve-progress';

/** Where an Expose run currently is: the TypeScript mirror of Rust's `ServePhase`
 *  (`src/mobile.rs`), serialized internally-tagged on `phase`.
 *
 *  A discriminated union rather than a bare string, for the same two reasons the
 *  updater's is one: the phase's data travels with it (only the approval phase
 *  has a URL, only `done` has one, only `failed` has a message), and `tsc` forces
 *  every consumer to handle every phase, so a variant added in Rust cannot render
 *  as a blank line here.
 *
 *  No phase carries a fraction, deliberately: not one step of this flow can
 *  honestly report one, so the surface spins rather than inventing a bar. */
export type TailscaleServeProgress =
  /** Claimed the slot, about to look for a CLI. */
  | { phase: 'starting' }
  /** Reading the tailnet address and the MagicDNS name. */
  | { phase: 'checking-tailnet' }
  /** `tailscale serve` is running. */
  | { phase: 'configuring' }
  /** Serve is not enabled on this tailnet, and the CLI is waiting for someone to
   *  approve it in a browser. `url` is the link IT printed (the node id in it is
   *  not reconstructable, and Rust has already checked it is an HTTPS Tailscale
   *  URL before offering it). The run keeps waiting and finishes by itself. */
  | { phase: 'awaiting-tailnet-approval'; url: string }
  /** Configured; waiting for something to answer on 443 (a first-run certificate
   *  takes a moment). */
  | { phase: 'waiting-for-https' }
  | { phase: 'done'; url: string }
  | { phase: 'failed'; message: string }
  | { phase: 'cancelled' };

/** Listen for a Tauri event. Returns an unlisten function. Only call when isTauri() is true. */
export function listen<T>(event: string, handler: (e: { payload: T }) => void): Promise<() => void> {
  const internals = window.__TAURI_INTERNALS__!;
  const callbackId = internals.transformCallback((raw: unknown) => {
    handler(raw as { payload: T });
  });
  return internals.invoke<number>('plugin:event|listen', {
    event,
    target: { kind: 'Any' },
    handler: callbackId,
  }).then((id) => {
    return () => {
      internals.invoke('plugin:event|unlisten', { event, eventId: id }).catch(() => {});
    };
  });
}
