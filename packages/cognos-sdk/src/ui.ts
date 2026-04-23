import { requestVoid } from './_fetch';
import { assertPlainObject } from './_validate';
import { preferences as prefsModule } from './preferences';
import { sse } from './sse';

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
   * Request navigation in the CognOS frontend.
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
};
