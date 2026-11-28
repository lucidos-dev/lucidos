import { applyAppearanceBoot } from './appearanceBoot';
import { wsLocalGet } from '../_storage';
import { THEME_MODE_ATTRIBUTE, THEME_MODE_STORAGE_KEY, statusBarColor } from '../appearance';

const SPLASH_BACKGROUND =
  '#145eb9 radial-gradient(125% 125% at 30% 22%, #2d83e0 0%, #0a4ea8 100%) no-repeat fixed';

type ThemeModeLogEvt = (label: string, info: unknown) => void;
declare global {
  interface Window {
    __themeModeLogEvt?: ThemeModeLogEvt;
  }
}

const boot = applyAppearanceBoot({
  styleReset: true,
  durationScale: true,
});

document.documentElement.style.background = SPLASH_BACKGROUND;

document.querySelector('meta[name="theme-color"]')?.setAttribute(
  'content',
  statusBarColor(document.documentElement.style.getPropertyValue('--header-bar-top'), boot.resolved),
);

try {
  const t0 = performance.now();
  window.__themeModeLogEvt = (label, info) => {
    fetch('api/v1/internal/client-log', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        category: 'theme-mode',
        code: label,
        message: label,
        data: Object.assign({ tMs: Math.round(performance.now() - t0) }, info),
      }),
      keepalive: true,
    }).catch(() => {});
  };
  window.__themeModeLogEvt('fouc', {
    raw: boot.raw,
    mode: boot.mode,
    resolved: boot.resolved,
    mqLight: boot.prefersLight,
  });
  window.addEventListener('pageshow', (e) => {
    window.__themeModeLogEvt?.('pageshow', {
      persisted: e.persisted,
      dataThemeMode: document.documentElement.getAttribute(THEME_MODE_ATTRIBUTE),
      rawNow: wsLocalGet(THEME_MODE_STORAGE_KEY),
      mqLightNow: matchMedia('(prefers-color-scheme: light)').matches,
    });
  });
} catch {
}
