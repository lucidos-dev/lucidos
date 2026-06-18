import { requestVoid } from './_fetch';
import { assertPlainObject } from './_validate';
import { preferences as prefsModule } from './preferences';
import { sse } from './sse';
import { Select, enhanceSelects } from './select';

const FONT_FAMILIES: Record<string, string> = {
  monospace: "'SF Mono', 'Fira Code', 'JetBrains Mono', Monaco, Consolas, monospace",
  system: "system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
  inter: "'Inter', system-ui, sans-serif",
  'jetbrains-mono': "'JetBrains Mono', monospace",
  'ibm-plex-mono': "'IBM Plex Mono', monospace",
};

const GOOGLE_FONT_URLS: Record<string, string> = {
  inter: 'https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&display=swap',
  'jetbrains-mono': 'https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;500;600;700&display=swap',
  'ibm-plex-mono': 'https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500;600;700&display=swap',
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

export const ui = {
  /** Fetch user preferences and apply theme, font, scale as CSS variables. */
  async applyPreferences(): Promise<void> {
    const prefs = await prefsModule.get();

    // Theme — resolve "system" via matchMedia
    let theme = prefs['theme'] || 'dark';
    if (theme === 'system') {
      theme = window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
    }
    const bg = theme === 'light' ? '#ffffff' : '#0b2342';
    document.documentElement.setAttribute('data-theme', theme);
    document.documentElement.style.setProperty('--bg-primary', bg);
    // Mirrors sdk-prefs.js — keeps <html> covered before/after the iframe's
    // stylesheet applies its bg rule (iOS WKWebView underlying white).
    document.documentElement.style.background = bg;

    // Font — load Google Fonts on demand, map to CSS value
    const fontKey = prefs['font-family'] || 'monospace';
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

    // Mirrors clampUiScale in preferences.ts — keep (75, 200, 12.5) in sync.
    const rawScale = prefs['ui-scale'] || prefs['text-size'] || prefs['font-size'];
    if (rawScale) {
      const legacy: Record<string, number> = { small: 100, medium: 112.5, large: 125 };
      let n = legacy[rawScale];
      if (n === undefined) n = parseFloat(rawScale);
      if (!isNaN(n)) {
        const snapped = Math.min(200, Math.max(75, Math.round(n / 12.5) * 12.5));
        document.documentElement.style.setProperty('--user-ui-scale', `${snapped}%`);
      }
    }
  },

  watchPreferences(): void {
    if (watchingPrefs) return;
    watchingPrefs = true;
    sse.on('PreferencesChanged', () => {
      ui.applyPreferences();
    });
    sse.connect();
    // A live OS light/dark flip under a `system` theme preference does NOT emit
    // PreferencesChanged, so also re-apply on the media-query change — matching
    // the host shell (preferences.ts → syncSystemThemeListener). `applyPreferences`
    // stays the source of truth for which theme actually applies (for an explicit
    // light/dark preference it re-applies the same value — a visual no-op). Skipped
    // on iOS, whose WKWebView fires bogus
    // prefers-color-scheme events; there `system` resolves once per applyPreferences().
    if (typeof window !== 'undefined' && typeof window.matchMedia === 'function' && !isIOSAgent()) {
      window.matchMedia('(prefers-color-scheme: light)').addEventListener('change', () => {
        ui.applyPreferences();
      });
    }
  },

  /**
   * Request navigation in the Lucidos frontend.
   * Calls POST /api/v1/ui/navigate, which emits a NavigationRequested event
   * that the frontend subscribes to via SSE.
   */
  navigate(target: string, params: Record<string, string> = {}): Promise<void> {
    assertPlainObject('params', params);
    return requestVoid('/ui/navigate', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ target, params }),
    });
  },

  /**
   * Open a fresh chat thread, optionally prefilling the compose textarea.
   * The user must click Send — this never auto-submits. If the user is on
   * an app/trigger/settings panel it closes the overlay first; if a thread
   * is focused it drops the focus so the compose lands on a new thread.
   */
  startThread(opts?: { prompt?: string }): Promise<void> {
    const params: Record<string, string> = {};
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
