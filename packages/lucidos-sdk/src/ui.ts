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
    document.documentElement.setAttribute('data-theme', theme);

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

    // Scale — handle legacy named sizes
    const rawScale = prefs['ui-scale'] || prefs['text-size'] || prefs['font-size'];
    if (rawScale) {
      const legacyMap: Record<string, string> = { small: '100%', medium: '113%', large: '125%' };
      const value = legacyMap[rawScale] || (/^\d+$/.test(rawScale) ? `${rawScale}%` : null);
      if (value) {
        document.documentElement.style.setProperty('--user-ui-scale', value);
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
  },

  /**
   * Request navigation in the Lucidos frontend.
   * Calls POST /api/v1/ui/navigate, which emits a NavigationRequested event
   * that the frontend subscribes to via SSE.
   */
  navigate(target: string, params: Record<string, string> = {}): Promise<void> {
    assertPlainObject('params', params);
    return requestVoid('/api/v1/ui/navigate', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ target, params }),
    });
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
