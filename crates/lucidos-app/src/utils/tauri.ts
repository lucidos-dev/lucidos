import { recordIpcOutcome } from './ipcHealth';

declare global {
  interface Window {
    __TAURI_INTERNALS__?: {
      invoke: <T>(cmd: string, args?: Record<string, unknown>, options?: unknown) => Promise<T>;
      transformCallback: (callback: (response: unknown) => void, once?: boolean) => number;
      /** Injected per main frame by the Tauri runtime: which window / webview
       *  this page is. `listen` needs the window label to scope itself. */
      metadata?: {
        currentWindow?: { label?: string };
        currentWebview?: { label?: string };
      };
    };
    /** Injected by the Tauri app on page load: CalVer version at build time. */
    __LUCIDOS_APP_VERSION__?: string;
  }
}

/** Invoke a Tauri command via IPC. Only call when isTauri() is true.
 *
 *  Every command goes through here, so this is where the health of the bridge
 *  itself is observed. Outcomes feed `recordIpcOutcome`, which writes a durable
 *  `[Client/ipc]` line to the engine log when calls start failing and another
 *  when they recover. A call site is therefore free to swallow its own
 *  rejection without costing the signal. See utils/ipcHealth. The returned
 *  promise is untouched: reporting must not change what callers see. */
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
 * `titleBarStyle: "Overlay"` the webview paints the reclaimed title-bar band.
 * This colors the behind-the-webview fallback, so that band reads blue rather
 * than black before the page paints. `color` is a CSS hex string. Sets the
 * window layer only, so the page background is untinted. Only call when
 * isTauri() is true. */
export function setTitlebarColor(color: string): Promise<void> {
  return invoke('set_titlebar_color', { color });
}

/**
 * Centre the macOS traffic lights on a header bar `barHeightPx` tall (lib.rs
 * `set_traffic_light_offset`). The bar height can only come from here: it is
 * `--titlebar-inset` plus `--app-header-height`, and the second is rem-authored,
 * so it is the user's UI scale that decides it. The shell remembers the last
 * value for the next cold launch. Only call when isTauri() is true, and only on
 * a build that stamps `data-titlebar-overlay` (see store/actions/trafficLights).
 */
export function setTrafficLightOffset(barHeightPx: number): Promise<void> {
  return invoke('set_traffic_light_offset', { barHeightPx });
}

/** Tell the shell this document is about to paint, so it can show the window it
 *  deliberately kept hidden at launch (lib.rs `window_ready_to_show`). Showing
 *  it from `setup()` instead leaves it on screen while the webview loads, which
 *  reads as a frame of bare window tint.
 *
 *  ONE-SHOT per document, which is the load-bearing part. Both callers repeat,
 *  and re-showing a window the user has since dismissed to the menu bar would
 *  be a bug. The flag lives here rather than at either call site because the
 *  two cover different boot paths and must share one shot.
 *
 *  Best-effort telemetry carve-out (.claude/rules/frontend.md): a toast would
 *  be wrong because nothing here is user-initiated, and `setup()`'s fallback
 *  timer shows the window a few seconds in regardless. Only call when isTauri()
 *  is true. */
let readyToShowSignalled = false;
export function windowReadyToShow(): void {
  if (readyToShowSignalled) return;
  readyToShowSignalled = true;
  invoke('window_ready_to_show').catch((e) => console.warn('[window] ready-to-show failed', e));
}

/**
 * Start a native drag of the calling window. Used by `useWindowDragRegion` once
 * the pointer crosses the drag threshold over a non-interactive area of the
 * title-bar band. An app command, which is always allowed, unlike the
 * ACL-blocked `data-tauri-drag-region`. Only call when isTauri() is true. */
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
 * Point the CALLING window's native cursor at a CSS cursor keyword (Rust
 * `cursor::set_window_cursor`). The keyword travels verbatim, because the one
 * table that turns it into a native icon lives in `src/cursor.rs` and this side
 * deliberately holds none.
 *
 * Called by the reconciler in `utils/nativeCursor.ts`, which is where the whole
 * mechanism is explained. Only call when isTauri() is true. */
export function setWindowCursor(cursor: string): Promise<void> {
  return invoke('set_window_cursor', { cursor });
}

/**
 * Name the CALLING window (lib.rs `set_window_title`), so the macOS Window menu
 * tells two windows apart by the workspace each is showing.
 *
 * Compose the string with `utils/windowTitle.ts` rather than here, and call it
 * through `pushNativeWindowTitle`, which de-duplicates. The title is invisible
 * in the window itself under the overlay title bar; it reaches the Window menu,
 * Mission Control and the window switcher. Only call when isTauri() is true. */
export function setWindowTitle(title: string): Promise<void> {
  return invoke('set_window_title', { title });
}

/**
 * Show a native macOS notification banner via the app's own
 * `show_native_notification` command (notifications.rs). Rust drives Apple's
 * `UNUserNotificationCenter` and captures the tap via a delegate.
 * `tauri-plugin-notification` and `mac-notification-sys` are not options: both
 * sit on the deprecated `NSUserNotification` API, which no longer delivers on
 * recent macOS.
 *
 * `deepLink` is the SW-message shape. On tap the command emits
 * `native-notification-tapped` with it, which the page routes through the same
 * dispatchDeepLink the web-push tap uses. Only call when isTauri() is true.
 */
export async function showNativeNotification(opts: {
  title: string;
  body: string;
  deepLink: Record<string, unknown>;
}): Promise<void> {
  // `link` (single word) matches the Rust command param name verbatim. The
  // caller stamps `workspace` into `deepLink`, which is what composes the UN
  // request identifier and lets the tap route back to the right workspace.
  await invoke('show_native_notification', {
    title: opts.title,
    body: opts.body,
    link: opts.deepLink,
  });
}

/**
 * Remove an already-delivered native macOS banner via the app's
 * `dismiss_native_notification` command (notifications.rs). The cross-device
 * dismiss counterpart of `showNativeNotification`: when a notification is read
 * elsewhere, the engine broadcasts `NativePushDismissRequested` and the
 * connected desktop app removes its banner.
 *
 * `notificationId === null` removes every delivered banner THIS workspace
 * raised, leaving other workspaces' banners on screen. `workspace` is the
 * caller's gateway slug, the same one `showNativeNotification` stamped into the
 * link. Both arms of the Rust side rebuild the composite request identifier
 * from it, so a bare id would match nothing. No-op in dev and off macOS. Only
 * call when isTauri() is true.
 */
export async function dismissNativeNotification(opts: {
  workspace: string | null;
  notificationId: string | null;
}): Promise<void> {
  // `workspace` / `id` match the Rust command params (`Option<String>`); a null
  // id → dismiss all of this workspace's.
  await invoke('dismiss_native_notification', {
    workspace: opts.workspace,
    id: opts.notificationId,
  });
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
 * The window it fronts is the calling page's, never `main`: fronting `main`
 * raises the wrong window whenever the flow finishes from a New Window or a
 * second workspace.
 *
 * Not a general "focus me" the page may call freely. A window that fronts
 * itself on a background event is a nuisance. Keep callers to "this page
 * started it, the user clicked seconds ago, fires once". Errors are swallowed:
 * there is no window to front on a failed IPC, and the connection itself
 * already succeeded. Only call when isTauri() is true. */
export function focusCallingWindow(): void {
  invoke('focus_calling_window').catch(() => {});
}

/**
 * Wake the native unread-indicator loop for an immediate recompute (Rust
 * `nudge_dock_badge`). The page calls this from its notification SSE handler.
 * The macOS count then updates the instant a notification is read, rather than
 * waiting for the desktop poll.
 *
 * The recompute always writes the menu-bar tray title, and the dock badge too
 * while a client window is open. It reads the gateway's fresh `unread-total`
 * aggregate, so this carries no count. Errors are swallowed, since neither
 * surface exists in dev or off macOS. Only call when isTauri() is true. */
export function nudgeDockBadge(): void {
  invoke('nudge_dock_badge').catch(() => {});
}

/**
 * Report whether the native main window is currently ACTIVE: focused and
 * on-screen, read live from AppKit by the Rust `get_native_window_active`.
 *
 * The page pulls this at startup to SEED its `native-window-active` cache
 * before registering the event listener. Tauri does not replay the transition
 * events to a late-registering listener, and the cache defaults to `true` (see
 * utils/nativeWindow.ts). Only call when isTauri() is true. */
export function getNativeWindowActive(): Promise<boolean> {
  return invoke<boolean>('get_native_window_active');
}

/**
 * Drain (and clear) the deep links from native-banner taps the page may not
 * have been listening for at emit time (webview reloaded / suspended-while-
 * trayed / client relaunched). Returned in SW-message shape so each routes
 * through the same dispatchDeepLink as a live tap / web-push tap. The drain is
 * atomic in Rust, so calling it from both the startup cold path and the
 * `native-notification-tapped` warm signal routes each tap exactly once.
 *
 * `workspace` is this page's gateway slug (null on a legacy engine with no
 * gateway). The stash is process-global while every window can sit on its own
 * workspace, so the Rust side hands back only the taps THIS workspace raised
 * (plus unattributable ones) and leaves the rest for the window their own
 * router is bringing up. Only call when isTauri() is true. */
export function takePendingNativeTaps(
  workspace: string | null,
): Promise<Record<string, unknown>[]> {
  return invoke<Record<string, unknown>[]>('take_pending_native_taps', { workspace });
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

/**
 * What id this window last used for `workspace`, or `null` if it has none.
 *
 * A reinstall re-buckets the webview's `localStorage` AND its cookie jar. The
 * window pairs again under a new name, and this native file is the only memory
 * of the old one. Reads only, so a caller can learn the old id, migrate the
 * engine's row, and commit the new one afterwards. Tauri only. */
export function previousDeviceId(workspace: string): Promise<string | null> {
  return invoke<string | null>('previous_device_id', { workspace });
}

/**
 * Record the id the gateway now names this window.
 *
 * Call only once the engine's row has actually moved. Writing before that would
 * discard the one memory a retry depends on. Tauri only. */
export function rememberDeviceId(workspace: string, id: string): Promise<void> {
  return invoke<void>('remember_device_id', { workspace, id });
}

// --- App auto-update (packaged desktop app) ---

/** A newer signed packaged build, and what is in it. */
export interface AppUpdateOffer {
  version: string;
  /** The release's notes as raw markdown, or null when the manifest carries
   *  none.
   *
   *  The ONLY way this client can say what a pending update contains: the
   *  offered version postdates the binary offering it, so it is absent from the
   *  changelog baked into that binary. Anything showing these must not fall back
   *  to the installed changelog, which would show the notes for the version
   *  already running. */
  notes: string | null;
}

/** Read what `check_app_update` actually answered.
 *
 *  This boundary is the one place that knows the command has had more than one
 *  return shape. The client BINARY owns the command and the frontend BUNDLE owns
 *  the call. They are installed separately, so a packaged client can run either
 *  half a release behind the other. Before 0.27.0 the command answered the bare
 *  version string; it now answers the version plus the release notes.
 *  {@link invoke} casts rather than checks. A frontend that trusted the
 *  annotation put the whole object where the version goes, and the update offer
 *  read `Lucidos [object Object] available`.
 *
 *  Both known shapes parse to one offer, so no surface downstream has to ask
 *  which client it is talking to. Anything else THROWS: null is the answer for
 *  "up to date", and a payload we cannot read is not that. The caller records
 *  the rejection on the check-error surface instead. */
function parseAppUpdateOffer(raw: unknown): AppUpdateOffer | null {
  if (raw === null || raw === undefined) return null;
  const offer = typeof raw === 'string' ? { version: raw } : (raw as Record<string, unknown>);
  const { version, notes } = offer;
  if (typeof version !== 'string' || version.trim() === '') {
    throw new Error(`check_app_update returned an unreadable offer: ${JSON.stringify(raw)}`);
  }
  return { version, notes: typeof notes === 'string' ? notes : null };
}

/** Check GitHub Releases for a newer signed packaged build. Returns the offer
 *  when one is available, else null. Drives the in-app update toast. Only call
 *  when isTauri() is true (no-op → null in dev). */
export async function checkAppUpdate(): Promise<AppUpdateOffer | null> {
  return parseAppUpdateOffer(await invoke<unknown>('check_app_update'));
}

/** Install the available packaged update and restart the WHOLE stack onto the
 *  new version: the launchd background service and the GUI client. On success
 *  the client re-execs and this promise never resolves. Otherwise it rejects
 *  with a string error. Progress arrives out-of-band on
 *  {@link APP_UPDATE_PROGRESS_EVENT}, so this promise says nothing until it is
 *  over. Only call when isTauri() is true. */
export function installAppUpdateAndRestart(): Promise<void> {
  return invoke('install_app_update_and_restart');
}

/** Tauri event carrying {@link AppUpdateProgress} frames while an update runs.
 *  Emitted by `src/updater.rs`; the name must match `PROGRESS_EVENT` there. */
export const APP_UPDATE_PROGRESS_EVENT = 'app-update-progress';

/** Where a packaged update run currently is — the TypeScript mirror of Rust's
 *  `AppUpdatePhase` (`src/updater.rs`), serialized internally-tagged on `phase`.
 *
 *  A discriminated union rather than a bare string, so the phase's data travels
 *  with it and `tsc` forces every consumer to handle every phase. `total` is
 *  null when the server declared no `Content-Length`. There is then no honest
 *  percentage, and the UI must show bytes alone rather than invent one. */
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
   *  message, because the two need different handling. `failed` is retryable
   *  and this is not: the recovery is a reinstall from the .dmg, and the page
   *  must not re-offer the update. */
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
 *  Tailnet state is read from the machine itself with no CLI. `cli_available`
 *  gates the action buttons and nothing else, so a Mac whose Tailscale works
 *  but has no CLI is still described accurately. */
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

/** localhost, LAN and Tailscale connect URLs for the engine (mirror of the Rust
 *  `ConnectInfo`). The LAN URL is derived client-side from `lan_ip`, `port` and
 *  the gateway bind: see `MobileAccessPage.tsx::lanRowState`. */
export interface ConnectInfo {
  port: number;
  localhost_url: string;
  lan_ip: string | null;
  tailscale: TailscaleInfo;
}

/** Open a URL in the system default browser (not the embedded webview).
 *
 *  Rejects when the OS launcher could not be STARTED, so callers owe the user a
 *  toast. It does NOT reject for a launcher that starts and then fails, such as
 *  no application registered for the scheme: the child is fire-and-forget on
 *  the Rust side, for the reason `open_in_default_browser` in `src/lib.rs`
 *  spells out.
 *
 *  Only call when isTauri() is true. */
export function openExternal(url: string): Promise<void> {
  return invoke('open_url_external', { url });
}

/** Where a saved download landed (mirror of the Rust `SavedDownload`). `dir` is
 *  the folder to open; `path` is the file inside it, which may carry a ` (1)`
 *  counter the caller did not ask for. */
export interface SavedDownload {
  dir: string;
  path: string;
}

/** Write `contents` into the OS downloads folder as `filename` (lib.rs
 *  `save_to_downloads`), and report where it landed.
 *
 *  The desktop client cannot download through the webview: wry attaches a
 *  download delegate only when the app registers a download handler, and this
 *  app registers none, so an `<a download>` click is silently dropped. Saving
 *  through the command is what makes the file exist, and its answer is what
 *  lets a toast name and open the folder.
 *
 *  Rejects with a string when the folder cannot be resolved, the name is not a
 *  leaf name, or the write fails. Callers owe the user a toast. Only call when
 *  isTauri() is true. */
export function saveToDownloads(filename: string, contents: string): Promise<SavedDownload> {
  return invoke<SavedDownload>('save_to_downloads', { filename, contents });
}

/** Show a workspace in a window (lib.rs `show_workspace_window`). The desktop
 *  half of `openWorkspaceIn`, which is the only thing that should call it.
 *
 *  The shell decides WHICH window: it focuses one already on the workspace,
 *  points the calling window at it when that window is on the picker, or opens
 *  a new one. Only the shell can see every window, so only the shell can choose.
 *
 *  Takes the workspace SLUG, never a URL, and the shell composes the URL itself
 *  on the calling window's own origin. A `window-*` webview carries the full IPC
 *  grant on the gateway origin (ADR 0028). Rejects with a string when the slug is
 *  not one the gateway serves, or the window could not be built.
 *
 *  Only call when isTauri() is true. */
export function showWorkspaceInNativeWindow(workspace: string): Promise<void> {
  return invoke('show_workspace_window', { workspace });
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
 *  Resolves only when the whole run is OVER, and a run legitimately waits
 *  minutes for a tailnet approval. Everything the user sees in between arrives
 *  on {@link TAILSCALE_SERVE_PROGRESS_EVENT}. Await it silently and the button
 *  looks dead. */
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
 *  A discriminated union rather than a bare string, for the same two reasons
 *  the updater's is one. The phase's data travels with it. And `tsc` forces
 *  every consumer to handle every phase, so a variant added in Rust cannot
 *  render as a blank line here.
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
  /** Serve is not enabled on this tailnet, and the CLI is waiting for someone
   *  to approve it in a browser. `url` is the link IT printed: the node id in
   *  it is not reconstructable, and Rust has already checked it is an HTTPS
   *  Tailscale URL. The run keeps waiting and finishes by itself. */
  | { phase: 'awaiting-tailnet-approval'; url: string }
  /** Configured; waiting for something to answer on 443 (a first-run certificate
   *  takes a moment). */
  | { phase: 'waiting-for-https' }
  | { phase: 'done'; url: string }
  | { phase: 'failed'; message: string }
  | { phase: 'cancelled' };

/** This window's Tauri label (`main`, `window-<n>`), read from the metadata the
 *  runtime injects into every main frame. `null` off Tauri, and defensively when
 *  the shape is missing. Exported for {@link listen}'s target and for tests. */
export function currentWindowLabel(): string | null {
  const label = window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label;
  return typeof label === 'string' && label.length > 0 ? label : null;
}

/**
 * Listen for a Tauri event **addressed to this window**. Returns an unlisten
 * function. Only call when isTauri() is true.
 *
 * **The registered target is load-bearing, and `Any` is a trap.** Tauri's
 * dispatch does `*listener_target == EventTarget::Any || filter(target)`
 * (`match_any_or_filter` in tauri's `event/listener.rs`), so a listener
 * registered as `Any` matches unconditionally and receives every
 * `emit_to(other_label, ...)` in the process. That defeats all three of the
 * app's targeted emits: `native-window-active`, `native-notification-tapped`,
 * and the `panel-*` panel-preview events.
 *
 * Registering as `AnyLabel` instead costs nothing on the broadcast path. A
 * plain `app.emit(...)` dispatches with no filter, which that same expression
 * passes, so the progress streams still arrive. Falls back to `Any` when the
 * label cannot be read, rather than to a listener that hears nothing.
 */
export function listen<T>(event: string, handler: (e: { payload: T }) => void): Promise<() => void> {
  const internals = window.__TAURI_INTERNALS__!;
  const label = currentWindowLabel();
  const callbackId = internals.transformCallback((raw: unknown) => {
    handler(raw as { payload: T });
  });
  return internals.invoke<number>('plugin:event|listen', {
    event,
    target: label === null ? { kind: 'Any' } : { kind: 'AnyLabel', label },
    handler: callbackId,
  }).then((id) => {
    return () => {
      internals.invoke('plugin:event|unlisten', { event, eventId: id }).catch(() => {});
    };
  });
}
