import { requestVoid } from './_fetch';
import { assertPlainObject, assertString } from './_validate';
import { wsLocalGet } from './_storage';
import { preferences as prefsModule } from './preferences';
import { sse } from './sse';
import { Select, enhanceSelects } from './select';
import type { NavigateTarget, NavigateUi } from './notifications';

/** Params for `lucidos.ui.navigate` — the `NavigateUi` payload minus `target`
 *  (which is the first argument). Carries the generated `settings_view`
 *  (`SettingsViewTarget`) so the full Settings sub-section set is type-checked
 *  and discoverable from the SDK, in lockstep with the engine `navigate_ui` tool. */
export type NavigateParams = Omit<NavigateUi, 'target'>;

const FONT_FAMILIES: Record<string, string> = {
  monospace: "ui-monospace, SFMono-Regular, 'SF Mono', Menlo, 'Fira Code', 'JetBrains Mono', Monaco, Consolas, monospace",
  system: "system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
  inter: "'Inter', system-ui, sans-serif",
  'jetbrains-mono': "'JetBrains Mono', monospace",
  'ibm-plex-mono': "'IBM Plex Mono', monospace",
  'fira-code': "'Fira Code', monospace",
};

const GOOGLE_FONT_URLS: Record<string, string> = {
  inter: 'https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&display=swap',
  'jetbrains-mono': 'https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;500;600;700&display=swap',
  'ibm-plex-mono': 'https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500;600;700&display=swap',
  'fira-code': 'https://fonts.googleapis.com/css2?family=Fira+Code:wght@400;500;600;700&display=swap',
};

// Programming ligatures: OFF for text, ON for code, gated to fonts that ship
// them (Fira Code). Every other font resolves BOTH to `normal`, leaving its
// rendering unchanged. Mirrors FONT_FEATURES in the host shell
// (crates/lucidos-app/src/store/actions/preferences.ts) and the FOUC scripts
// (index.html, crates/lucidos-engine/src/api/sdk_prefs.rs). The engine-served
// api/sdk_iframe.css is the only consumer: `html, input, textarea, select,
// button` take the text value, `code, pre, kbd, samp` the code one.
//
// The off value is the ZEROS, never `normal`: `liga` and `calt` are default-ON
// in CSS, so `normal` means "the font's defaults" and renders identically to
// `1`, leaving Fira Code's `calt` free to re-space a typed `...` into what reads
// as two dots (tonsky/FiraCode#1561). Form controls are named explicitly
// because the UA stylesheet's `font` shorthand resets this property on them, so
// a `<textarea>` never inherits it. See preferences.ts for the full why.
type FontFeaturePair = { text: string; code: string };
const FONT_FEATURES_DEFAULT: FontFeaturePair = { text: 'normal', code: 'normal' };
const FONT_FEATURES: Record<string, FontFeaturePair> = {
  'fira-code': { text: '"liga" 0, "calt" 0', code: '"liga" 1, "calt" 1' },
};

const loadedFonts = new Set<string>();
let watchingPrefs = false;

/**
 * iOS / iPadOS detection — mirrors `crates/lucidos-app/src/utils/platform.ts`.
 * Exported for tests; `nav` defaults to the global navigator. Used to skip the
 * live OS-appearance listener on iOS, whose WKWebView fires bogus
 * `prefers-color-scheme` change events (telemetry-confirmed theme flashing).
 */
export function isIOSAgent(
  nav: { userAgent: string; platform?: string; maxTouchPoints?: number } | undefined =
    typeof navigator !== 'undefined' ? navigator : undefined,
): boolean {
  if (!nav) return false;
  return /iPad|iPhone|iPod/.test(nav.userAgent) ||
    (nav.platform === 'MacIntel' && (nav.maxTouchPoints ?? 0) > 1);
}

type ThemePref = 'light' | 'dark' | 'system';
const VALID_THEMES: readonly ThemePref[] = ['light', 'dark', 'system'];

/**
 * Resolve which theme preference applies, mirroring the host shell's
 * `currentPreference()` (crates/lucidos-app/src/store/actions/preferences.ts)
 * and the synchronous `sdk-prefs.js` FOUC script. Precedence:
 *   1. the server-provided value, when present and valid;
 *   2. else the `lucidos-theme` localStorage value `sdk-prefs.js` read;
 *   3. else the `data-theme` attribute `sdk-prefs.js` already applied;
 *   4. else the hard default `'dark'` (matching `sdk-prefs.js`).
 *
 * Load-bearing invariant: a MISSING server theme must NEVER clobber the value
 * the synchronous client resolver already settled on. The previous
 * `prefs['theme'] || 'dark'` violated this — a device with no server-scoped
 * theme (e.g. an iPhone PWA that only stored `ui-scale`) flipped every app
 * iframe to dark even when localStorage said light.
 *
 * `getAttr` is a thunk so the DOM is read only as a last resort — keeping the
 * common path side-effect-free and the function unit-testable without a DOM.
 * Returns a raw preference; the caller resolves `system` via matchMedia.
 */
export function resolveThemePreference(
  server: string | undefined,
  local: string | null,
  getAttr: () => string | null,
): ThemePref {
  const valid = VALID_THEMES as readonly string[];
  if (server && valid.includes(server)) return server as ThemePref;
  if (local && valid.includes(local)) return local as ThemePref;
  const attr = getAttr();
  return attr === 'light' || attr === 'dark' ? attr : 'dark';
}

let confirmCounter = 0;
const pendingConfirms = new Map<string, (value: boolean) => void>();
let confirmListenerInstalled = false;

function installConfirmListener() {
  if (confirmListenerInstalled) return;
  confirmListenerInstalled = true;
  window.addEventListener('message', (event: MessageEvent) => {
    const data = event.data as { type?: unknown; id?: unknown; ok?: unknown } | null;
    if (!data || typeof data !== 'object') return;
    if (data.type !== 'lucidos:ui:confirm:result') return;
    if (typeof data.id !== 'string') return;
    const resolver = pendingConfirms.get(data.id);
    if (!resolver) return;
    pendingConfirms.delete(data.id);
    resolver(data.ok === true);
  });
}

export interface ConfirmOptions {
  title?: string;
  message: string;
  okLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
}

/** Toast severity — matches the host shell's toast types. */
export type ToastType = 'success' | 'info' | 'warning' | 'error';
const TOAST_TYPES: readonly ToastType[] = ['success', 'info', 'warning', 'error'];

export interface ToastOptions {
  /** Auto-dismiss after this many ms. Omit for the host default (errors/warnings
   *  stay until dismissed; success/info auto-close). */
  durationMs?: number;
  /** false = hide the close (X) button. Default true. */
  dismissable?: boolean;
  /** Stable key for in-place replacement. A later toast with the same key
   *  updates the existing toast (message/type/etc.) instead of stacking a new
   *  one — e.g. an 'Opening…' toast becoming 'Opened'. */
  key?: string;
  /** true = show an indeterminate "work in progress" spinner in place of the
   *  severity icon. Pair it with a `key` so a later keyed toast replaces the
   *  spinning one with the outcome (or call `dismissToast(key)` when the work
   *  finishes with nothing to say). Indeterminate only: there is no app-facing
   *  percentage. */
  spinning?: boolean;
}

export interface PromptOptions {
  /** Required. The question/instruction shown above the input. Plain text. */
  message: string;
  /** Optional heading rendered above the message. */
  title?: string;
  /** Prefilled input value. */
  defaultValue?: string;
  /** Placeholder shown when the input is empty. */
  placeholder?: string;
  /** OK button label. Default "OK". */
  okLabel?: string;
  /** Cancel button label. Default "Cancel". */
  cancelLabel?: string;
  /** Render a multi-line textarea instead of a single-line input. Default false. */
  multiline?: boolean;
}

let promptCounter = 0;
const pendingPrompts = new Map<string, (value: string | null) => void>();
let promptListenerInstalled = false;

function installPromptListener() {
  if (promptListenerInstalled) return;
  promptListenerInstalled = true;
  window.addEventListener('message', (event: MessageEvent) => {
    const data = event.data as { type?: unknown; id?: unknown; value?: unknown } | null;
    if (!data || typeof data !== 'object') return;
    if (data.type !== 'lucidos:ui:prompt:result') return;
    if (typeof data.id !== 'string') return;
    const resolver = pendingPrompts.get(data.id);
    if (!resolver) return;
    pendingPrompts.delete(data.id);
    // A string is an OK with the entered text; anything else (cancel/esc) is null.
    resolver(typeof data.value === 'string' ? data.value : null);
  });
}

/** What `lucidos.ui.previewFile` shows.
 *
 *  Deliberately the `file` navigate target's own field names, `snake_case`
 *  included, so one object literal drives both calls and an app promotes a
 *  glance into a navigation by swapping the verb:
 *
 *  ```js
 *  const at = { file_path: `repo:${repoId}:file:src/main.rs`, line: 510, line_end: 520 };
 *  await lucidos.ui.previewFile(at);        // glance, your app stays put
 *  await lucidos.ui.navigate('file', at);   // leave for the Files panel
 *  ```
 */
export interface FilePreviewParams {
  /** The same forms `navigate('file', …)` accepts: a workspace data path
   *  (`artifacts/…`, `knowhow/…`, `apps/…`, `triggers/…`, `system-knowhow/…`, or
   *  a bare name, which is treated as an artifact), or a repo-encoded path for a
   *  file in a registered repository clone, either at its current `HEAD`
   *  (`repo:<repoId>:file:<repo-relative path>`) or at a branch, tag or sha you
   *  name (`repo:<repoId>:file#<ref>:<repo-relative path>`).
   *
   *  Naming the revision matters here: the modal may be showing a repository the
   *  Files panel is not bound to, so it cannot fall back to whatever branch that
   *  panel is on. A file a coding agent has edited is on that agent's branch,
   *  not at `HEAD`. */
  file_path: string;
  /** 1-based first line to highlight and scroll to. */
  line?: number;
  /** Inclusive last line of the range; omit to highlight a single line. */
  line_end?: number;
}

let previewCounter = 0;
const pendingPreviews = new Map<string, (result: { ok: boolean; error?: string }) => void>();
let previewListenerInstalled = false;

function installPreviewListener() {
  if (previewListenerInstalled) return;
  previewListenerInstalled = true;
  window.addEventListener('message', (event: MessageEvent) => {
    const data = event.data as { type?: unknown; id?: unknown; ok?: unknown; error?: unknown } | null;
    if (!data || typeof data !== 'object') return;
    if (data.type !== 'lucidos:ui:preview-file:result') return;
    if (typeof data.id !== 'string') return;
    const resolver = pendingPreviews.get(data.id);
    if (!resolver) return;
    pendingPreviews.delete(data.id);
    resolver({
      ok: data.ok === true,
      error: typeof data.error === 'string' ? data.error : undefined,
    });
  });
}

/** Where an external http(s) link goes when tapped in an installed iOS PWA.
 *  Mirrors `ExternalLinkTarget` in
 *  `crates/lucidos-app/src/store/actions/preferences.ts`. */
export type ExternalLinkTarget = 'safari' | 'ask' | 'in-app';

const EXTERNAL_LINK_TARGETS: readonly ExternalLinkTarget[] = ['safari', 'ask', 'in-app'];

/** The last-seen `external_link_target`, refreshed by `applyPreferences` (and
 *  therefore by `watchPreferences`, which re-runs it on `PreferencesChanged`).
 *
 *  It is a cache rather than a fetch because `openExternal` must decide
 *  SYNCHRONOUSLY: the `ask` mode calls `navigator.share`, which requires
 *  transient user activation, and any `await` between the click and the call
 *  spends it. `null` means "not fetched yet", which routes to the host instead
 *  of guessing. */
let externalLinkTargetCache: ExternalLinkTarget | null = null;

function cacheExternalLinkTarget(raw: string | undefined): void {
  externalLinkTargetCache = EXTERNAL_LINK_TARGETS.includes(raw as ExternalLinkTarget)
    ? raw as ExternalLinkTarget
    : 'safari';
}

/** In-flight prime, so a themed app calling `applyPreferences` and the
 *  load-time prime below don't each fetch. */
let primingExternalLinkTarget: Promise<void> | null = null;

/** Warm {@link externalLinkTargetCache} without going through
 *  `applyPreferences`.
 *
 *  Needed because `applyPreferences` is OPTIONAL: an app shipping its own
 *  complete visual identity legitimately never calls it, yet still gets the
 *  SDK's delegated link handler (installed unconditionally by `browser.ts`).
 *  Left to theming alone, those apps would hold a `null` cache forever, take the
 *  host path on every link, and silently ignore the user's "Ask" choice, since
 *  by then the activation `navigator.share` needs is long gone.
 *
 *  Called at load from `browser.ts`, and only inside an installed iOS PWA, the
 *  one place the cache is ever read. Best-effort: a failure leaves the cache
 *  null, which falls back to the host path (the same behaviour as before this
 *  preference existed).
 *
 *  Not live: an app that never calls `watchPreferences` keeps the mode it saw at
 *  load until the next reload. Subscribing SSE from every app iframe to catch a
 *  rare mid-session change is not worth the connection. */
export function primeExternalLinkTarget(): Promise<void> {
  if (externalLinkTargetCache !== null) return Promise.resolve();
  if (!inIOSStandalone()) return Promise.resolve();
  primingExternalLinkTarget ??= prefsModule.get()
    .then((prefs) => { cacheExternalLinkTarget(prefs['external_link_target']); })
    .finally(() => { primingExternalLinkTarget = null; });
  return primingExternalLinkTarget;
}

/** Whether this frame is inside an installed iOS PWA. The app iframe inherits
 *  the host's display mode, so the same check the host makes works here. */
function inIOSStandalone(): boolean {
  if (typeof navigator === 'undefined' || typeof window === 'undefined') return false;
  if (!isIOSAgent()) return false;
  return window.matchMedia?.('(display-mode: standalone)').matches === true
    || (navigator as Navigator & { standalone?: boolean }).standalone === true;
}

const HTTP_SCHEME_RE = /^https?:\/\//i;

export const ui = {
  /** Fetch user preferences and apply theme, font, scale as CSS variables. */
  async applyPreferences(): Promise<void> {
    const prefs = await prefsModule.get();

    // Theme — prefer the value the synchronous sdk-prefs.js resolver already
    // applied (server → localStorage → data-theme) over a hard default, so a
    // missing server-scoped theme can't flip the iframe to dark. Then resolve
    // "system" via matchMedia.
    let theme: string = resolveThemePreference(
      prefs['theme'],
      wsLocalGet('lucidos-theme'),
      () => document.documentElement.getAttribute('data-theme'),
    );
    if (theme === 'system') {
      theme = window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
    }
    const bg = theme === 'light' ? '#ffffff' : '#07172e';
    document.documentElement.setAttribute('data-theme', theme);
    document.documentElement.style.setProperty('--bg-primary', bg);
    // Mirrors sdk-prefs.js — keeps <html> covered before/after the iframe's
    // stylesheet applies its bg rule (iOS WKWebView underlying white).
    document.documentElement.style.background = bg;

    // Font — load Google Fonts on demand, map to CSS value. Fall back to the
    // `lucidos-font-family` localStorage value sdk-prefs.js read before the
    // hard default, so a missing server value doesn't reset the client font.
    const fontKey = prefs['font-family'] || wsLocalGet('lucidos-font-family') || 'monospace';
    const googleUrl = GOOGLE_FONT_URLS[fontKey];
    if (googleUrl && !loadedFonts.has(fontKey)) {
      loadedFonts.add(fontKey);
      const link = document.createElement('link');
      link.rel = 'stylesheet';
      link.href = googleUrl;
      document.head.appendChild(link);
    }
    const fontValue = FONT_FAMILIES[fontKey] || FONT_FAMILIES['monospace'];
    document.documentElement.style.setProperty('--font-ui', fontValue);
    const features = FONT_FEATURES[fontKey] || FONT_FEATURES_DEFAULT;
    document.documentElement.style.setProperty('--font-features-text', features.text);
    document.documentElement.style.setProperty('--font-features-code', features.code);

    // Mirrors clampUiScale in preferences.ts — keep (75, 200, 12.5) in sync.
    // Fall back to the `lucidos-ui-scale` localStorage value sdk-prefs.js read
    // so a missing server value doesn't drop the client-applied scale.
    const rawScale = prefs['ui-scale'] || prefs['text-size'] || prefs['font-size']
      || wsLocalGet('lucidos-ui-scale');
    if (rawScale) {
      const legacy: Record<string, number> = { small: 100, medium: 112.5, large: 125 };
      let n = legacy[rawScale];
      if (n === undefined) n = parseFloat(rawScale);
      if (!isNaN(n)) {
        const snapped = Math.min(200, Math.max(75, Math.round(n / 12.5) * 12.5));
        document.documentElement.style.setProperty('--user-ui-scale', `${snapped}%`);
      }
    }

    // Cache the external-link target for openExternal, which must resolve it
    // WITHOUT awaiting (see EXTERNAL_LINK_TARGET_CACHE).
    cacheExternalLinkTarget(prefs['external_link_target']);
  },

  watchPreferences(): void {
    if (watchingPrefs) return;
    watchingPrefs = true;
    // Best-effort live re-application: a transient prefs-fetch failure must not
    // surface as an unhandled rejection — the next PreferencesChanged / OS-theme
    // flip re-runs applyPreferences, and the app keeps its already-applied theme
    // in the meantime. Swallow with a warn so a developer can still see a
    // persistent failure.
    const reapply = () => {
      ui.applyPreferences().catch((err) => {
        console.warn('[lucidos-sdk] live preference re-apply failed:', err);
      });
    };
    sse.on('PreferencesChanged', reapply);
    sse.connect();
    // A live OS light/dark flip under a `system` theme preference does NOT emit
    // PreferencesChanged, so also re-apply on the media-query change — matching
    // the host shell (preferences.ts → syncSystemThemeListener). `applyPreferences`
    // stays the source of truth for which theme actually applies (for an explicit
    // light/dark preference it re-applies the same value — a visual no-op). Skipped
    // on iOS, whose WKWebView fires bogus
    // prefers-color-scheme events; there `system` resolves once per applyPreferences().
    if (typeof window !== 'undefined' && typeof window.matchMedia === 'function' && !isIOSAgent()) {
      window.matchMedia('(prefers-color-scheme: light)').addEventListener('change', reapply);
    }
  },

  /**
   * Request navigation in the Lucidos frontend.
   * Calls POST /api/v1/ui/navigate, which emits a NavigationRequested event
   * that the frontend subscribes to via SSE.
   */
  navigate(target: NavigateTarget, params: NavigateParams = {}): Promise<void> {
    assertPlainObject('params', params);
    return requestVoid('/ui/navigate', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ target, params }),
    });
  },

  /**
   * Open a URL outside Lucidos, honouring the user's external-link preference.
   *
   * Use this instead of `window.open` for any link out of your app. From inside
   * an app iframe `window.open` cannot escape an installed iOS PWA: WebKit
   * renders it in the PWA's in-app web view, which has no address bar, no tabs
   * and no shared Safari session. Anchors are already handled for you (the SDK
   * delegates `<a href="https://…">` clicks automatically); this is the
   * programmatic equivalent for links opened from JS.
   *
   * MUST be called synchronously from the user's click/tap handler. In the
   * user's "Ask" mode this opens the OS share sheet via `navigator.share`, which
   * the browser refuses without transient user activation, and any `await`
   * before this call spends that activation. Do the async work first, then call
   * this from a later gesture.
   *
   * Non-http(s) URLs (`mailto:`, `tel:`) are handed to the platform unchanged.
   * Resolves once the open has been dispatched; a user dismissing the share
   * sheet is a normal resolve, not a rejection.
   */
  openExternal(url: string): Promise<void> {
    assertString('url', url);
    // Share only where the preference can apply and only for a web page. Off
    // iOS, and for `mailto:` / `tel:`, the host path is always right.
    if (
      inIOSStandalone()
      && HTTP_SCHEME_RE.test(url)
      && externalLinkTargetCache === 'ask'
      && typeof navigator.share === 'function'
    ) {
      // Deliberately NOT routed through the host: user activation does not
      // survive the hop (the host's navigate goes over HTTP and lands via SSE),
      // so a host-side share would be refused on every app link. The iframe is
      // same-origin, so the `web-share` permissions-policy default of `self`
      // covers this frame.
      return navigator.share({ url }).catch((err: unknown) => {
        // The user closing the sheet chose "none of these"; honour it rather
        // than opening something anyway. Anything else means the sheet never
        // worked, so fall through to the host.
        if (err instanceof Error && err.name === 'AbortError') return;
        return ui.navigate('url', { url });
      });
    }
    return ui.navigate('url', { url });
  },

  /**
   * Open a fresh chat thread, optionally prefilling the compose textarea.
   * The user must click Send — this never auto-submits. If the user is on
   * an app/trigger/settings panel it closes the overlay first; if a thread
   * is focused it drops the focus so the compose lands on a new thread.
   */
  startThread(opts?: { prompt?: string }): Promise<void> {
    const params: NavigateParams = {};
    if (opts && opts.prompt !== undefined) {
      if (typeof opts.prompt !== 'string') {
        return Promise.reject(new TypeError('opts.prompt must be a string'));
      }
      if (opts.prompt.length > 0) params.prompt = opts.prompt;
    }
    return ui.navigate('new-chat', params);
  },

  /**
   * Show a confirmation dialog rendered by the host shell (above all app
   * content, themed by the user's preferences). Resolves to `true` on OK
   * and `false` on Cancel / Esc / backdrop click. If another confirm is
   * already showing, this one replaces it (the previous resolves `false`).
   */
  confirm(options: ConfirmOptions): Promise<boolean> {
    assertPlainObject('options', options);
    if (typeof options.message !== 'string' || options.message.length === 0) {
      return Promise.reject(new TypeError('options.message must be a non-empty string'));
    }
    // No parent (e.g. SDK loaded in a top-level window) — fall back to native
    // window.confirm so the API still works in standalone testing contexts.
    if (window.parent === window) {
      return Promise.resolve(window.confirm(options.message));
    }
    installConfirmListener();
    const id = `c${++confirmCounter}-${Date.now()}`;
    const payload = {
      title: typeof options.title === 'string' ? options.title : undefined,
      message: options.message,
      okLabel: typeof options.okLabel === 'string' && options.okLabel.length > 0 ? options.okLabel : 'Confirm',
      cancelLabel: typeof options.cancelLabel === 'string' && options.cancelLabel.length > 0 ? options.cancelLabel : 'Cancel',
      danger: options.danger === true,
    };
    return new Promise<boolean>((resolve) => {
      // Bound the wait so a host crash or dropped reply can't leak the Map entry forever.
      const timeout = setTimeout(() => {
        if (pendingConfirms.delete(id)) resolve(false);
      }, 60_000);
      pendingConfirms.set(id, (value) => {
        clearTimeout(timeout);
        resolve(value);
      });
      window.parent.postMessage({ type: 'lucidos:ui:confirm', id, payload }, '*');
    });
  },

  /**
   * Show a transient toast rendered by the host shell (above all app content,
   * themed by the user's preferences). Fire-and-forget — there is no result.
   *
   * Only the serializable subset of the host toast is exposed: `message`, the
   * `type` severity, and `opts` (`durationMs`, `dismissable`, `key`,
   * `spinning`). The host's action-button callbacks can't cross the postMessage
   * boundary, so they're intentionally not available here. An unknown `type`
   * degrades to `info`.
   */
  toast(message: string, type: ToastType = 'info', opts?: ToastOptions): void {
    if (typeof message !== 'string' || message.length === 0) {
      throw new TypeError('lucidos.ui.toast: message must be a non-empty string');
    }
    const safeType: ToastType = TOAST_TYPES.includes(type) ? type : 'info';
    const payload = {
      message,
      type: safeType,
      durationMs: opts && typeof opts.durationMs === 'number' ? opts.durationMs : undefined,
      dismissable: opts && typeof opts.dismissable === 'boolean' ? opts.dismissable : undefined,
      key: opts && typeof opts.key === 'string' && opts.key.length > 0 ? opts.key : undefined,
      spinning: opts && typeof opts.spinning === 'boolean' ? opts.spinning : undefined,
    };
    // No host parent (SDK loaded in a top-level window) — surface via console so
    // a standalone testing context still sees the feedback instead of silence.
    if (window.parent === window) {
      const line = `[lucidos.ui.toast:${safeType}] ${message}`;
      if (safeType === 'error') console.error(line);
      else if (safeType === 'warning') console.warn(line);
      else console.log(line);
      return;
    }
    window.parent.postMessage({ type: 'lucidos:ui:toast', payload }, '*');
  },

  /**
   * Take down a toast your app raised with `toast(…, { key })`. Fire-and-forget,
   * like `toast` itself. Use it for the case a keyed replacement can't express:
   * work that finishes with nothing left to say, e.g. a `spinning` "Syncing…"
   * toast that should just disappear when the SSE event lands.
   *
   * A key matching nothing is a silent no-op. Your app cannot know whether the
   * toast is still up (the user may have closed it, or its duration may have
   * expired), so "already gone" is the normal case, not an error.
   */
  dismissToast(key: string): void {
    if (typeof key !== 'string' || key.length === 0) {
      throw new TypeError('lucidos.ui.dismissToast: key must be a non-empty string');
    }
    // No host parent (SDK loaded in a top-level window): mirror the console
    // fallback in `toast()` so a standalone testing context sees both halves of
    // the exchange instead of a toast line with no matching dismissal.
    if (window.parent === window) {
      console.log(`[lucidos.ui.dismissToast] ${key}`);
      return;
    }
    window.parent.postMessage({ type: 'lucidos:ui:dismissToast', payload: { key } }, '*');
  },

  /**
   * Prompt for a line of text via a modal rendered by the host shell (above all
   * app content, themed by the user's preferences). Resolves to the entered
   * string on OK/Enter, or `null` on Cancel / Esc / backdrop click. If another
   * prompt is already showing, this one replaces it (the previous resolves
   * `null`). Use it instead of `window.prompt()`.
   */
  prompt(options: PromptOptions): Promise<string | null> {
    assertPlainObject('options', options);
    if (typeof options.message !== 'string' || options.message.length === 0) {
      return Promise.reject(new TypeError('options.message must be a non-empty string'));
    }
    // No parent (e.g. SDK loaded in a top-level window) — fall back to native
    // window.prompt so the API still works in standalone testing contexts.
    if (window.parent === window) {
      const def = typeof options.defaultValue === 'string' ? options.defaultValue : '';
      return Promise.resolve(window.prompt(options.message, def));
    }
    installPromptListener();
    const id = `p${++promptCounter}-${Date.now()}`;
    const payload = {
      title: typeof options.title === 'string' ? options.title : undefined,
      message: options.message,
      defaultValue: typeof options.defaultValue === 'string' ? options.defaultValue : undefined,
      placeholder: typeof options.placeholder === 'string' ? options.placeholder : undefined,
      okLabel: typeof options.okLabel === 'string' && options.okLabel.length > 0 ? options.okLabel : 'OK',
      cancelLabel: typeof options.cancelLabel === 'string' && options.cancelLabel.length > 0 ? options.cancelLabel : 'Cancel',
      multiline: options.multiline === true,
    };
    return new Promise<string | null>((resolve) => {
      // Bound the wait so a host crash or dropped reply can't leak the Map entry forever.
      const timeout = setTimeout(() => {
        if (pendingPrompts.delete(id)) resolve(null);
      }, 60_000);
      pendingPrompts.set(id, (value) => {
        clearTimeout(timeout);
        resolve(value);
      });
      window.parent.postMessage({ type: 'lucidos:ui:prompt', id, payload }, '*');
    });
  },

  /**
   * Show a file in a read-only modal rendered by the host, over your app,
   * WITHOUT navigating away. Use it for a citation in a report or a dashboard:
   * the reader glances at the code and carries on, instead of losing their place
   * in the Files panel and having to navigate back. The modal carries a link
   * that escalates into the full Files preview when they do want to leave.
   *
   * Takes the same locators as `navigate('file', …)` (a workspace data path, a
   * `repo:<repoId>:file:<path>` one, or `repo:<repoId>:file#<ref>:<path>` to
   * name a branch, tag or sha) and the same `line` / `line_end`, with the same
   * degradation: a line the file cannot honour (`0`, negative, fractional, past
   * the end, a format with no source view) opens the file at the top rather
   * than refusing it.
   *
   * Resolves once the preview is on screen, NOT when the reader dismisses it: a
   * glance can stay open for minutes, and your app is not blocked while it is.
   * Rejects when the host cannot show it, which makes the escalation a natural
   * fallback:
   *
   * ```js
   * try { await lucidos.ui.previewFile(at); }
   * catch { await lucidos.ui.navigate('file', at); }
   * ```
   *
   * Two things make it reject: your app is running with no host shell around it
   * (opened in its own tab, or the SDK loaded in a top-level page), and a
   * fullscreen element the host cannot render over. Both mean the same thing,
   * that nothing would appear, and the fallback above is what turns that into
   * something the reader can act on.
   *
   * A second call replaces a showing preview. Read-only: there is no editing in
   * the modal.
   */
  previewFile(params: FilePreviewParams): Promise<void> {
    assertPlainObject('params', params);
    if (typeof params.file_path !== 'string' || params.file_path.length === 0) {
      return Promise.reject(new TypeError('params.file_path must be a non-empty string'));
    }
    const payload = {
      file_path: params.file_path,
      line: typeof params.line === 'number' ? params.line : undefined,
      line_end: typeof params.line_end === 'number' ? params.line_end : undefined,
    };
    // No host shell around this window (an app opened in its own tab, or the SDK
    // loaded in a top-level page), so there is no modal to show and no reply to
    // wait for. Reject rather than quietly calling `navigate` here: that request
    // goes through the engine and lands in whichever OTHER window is running the
    // shell, so the reader clicking a citation would see nothing happen while a
    // different window navigated its Files panel behind their back, and the
    // promise would resolve as if it had worked. The escalation is the app
    // author's to make, from its own catch, where it is a deliberate "take me to
    // the workspace" rather than a silent side effect somewhere else.
    if (window.parent === window) {
      return Promise.reject(new Error(
        'lucidos.ui.previewFile: no host to show the preview (this app is not running inside Lucidos)',
      ));
    }
    installPreviewListener();
    const id = `v${++previewCounter}-${Date.now()}`;
    return new Promise<void>((resolve, reject) => {
      // Bounds a LOST reply, not the reader's time: the host answers as soon as
      // it has decided, so this only fires when nothing answered at all. Without
      // it a host crash would leak the Map entry forever.
      const timeout = setTimeout(() => {
        if (pendingPreviews.delete(id)) {
          reject(new Error('lucidos.ui.previewFile: the host did not respond'));
        }
      }, 60_000);
      pendingPreviews.set(id, (result) => {
        clearTimeout(timeout);
        if (result.ok) resolve();
        else reject(new Error(result.error || 'lucidos.ui.previewFile: the host refused the preview'));
      });
      window.parent.postMessage({ type: 'lucidos:ui:preview-file', id, payload }, '*');
    });
  },

  /** Themed dropdown — replaces native `<select>` so popups can be styled. */
  Select,

  /**
   * Enhance every `<select class="lucidos-select">` under `root` (default
   * `document`) with a themed dropdown. The native element stays in the DOM
   * (hidden) so existing form code keeps working — `change` fires on it and
   * its `value` mirrors the user's selection.
   */
  enhanceSelects,
};
