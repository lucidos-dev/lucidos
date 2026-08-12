/**
 * Entry point for the app shell's boot script, inlined into `index.html`'s
 * `<head>` by the `lucidos-appearance-boot` Vite plugin.
 *
 * The shared program plus the two things that are the SHELL's and not the
 * appearance contract's: the boot-splash gradient, and the theme telemetry
 * channel. Keeping them here is what lets the contract stay identical between
 * the shell and every app iframe.
 */
import { applyAppearanceBoot } from './appearanceBoot';
import { wsLocalGet } from '../_storage';

/**
 * The brand gradient, painted on the document canvas during boot.
 *
 * Covers two gaps the flat background does not: the WKWebView white flash on an
 * iOS PWA cold restart, and the iOS standalone bottom safe-area strip that the
 * fixed `inset: 0` `.boot-splash` element leaves bare.
 *
 * THE BASE COLOUR IS THE SEAM COLOUR, not the gradient's end colour. iOS fills
 * that bottom strip with the flat base and never with the gradient image, so the
 * base is what is actually seen there, butted directly against the gradient
 * above it. `#0a4ea8` (the 100% stop) made the strip a visibly darker band:
 * along the bottom edge the gradient is only 0.62 (at x=30%) to 0.84 (at x=100%)
 * of the way to it. `#145eb9` is the gradient's own colour at progress 0.70, the
 * mean across that edge, which leaves under 4% per channel anywhere along it.
 * Both radii are percentages of their own axis, so the figures are
 * aspect-independent and hold on every device.
 *
 * `dismissBootSplash()` (`utils/bootSplash.ts`) reverts this to
 * `var(--bg-primary)` once the splash is gone, so no blue lingers behind the
 * app's own bottom safe-area inset.
 */
const SPLASH_BACKGROUND =
  '#145eb9 radial-gradient(125% 125% at 30% 22%, #2d83e0 0%, #0a4ea8 100%) no-repeat fixed';

type ThemeLogEvt = (label: string, info: unknown) => void;
declare global {
  interface Window {
    __themeLogEvt?: ThemeLogEvt;
  }
}

const boot = applyAppearanceBoot({ styleReset: true });

// After the shared program, which sets the flat background: same end state as
// setting it inline there, and it keeps the gradient out of the contract.
document.documentElement.style.background = SPLASH_BACKGROUND;

// Always-on theme-flash telemetry: POST breadcrumbs to engine.log so a reported
// flash can be traced to the transition that preceded it. The bug being hunted
// (iOS WKWebView `matchMedia` returning a stale synchronous value at random
// post-FOUC moments) does not reproduce on demand, so the only way to catch it
// is to log every transition and inspect after the fact. Theme transitions fire
// three to five times per cold load, which is negligible noise.
// `window.__themeLogEvt` is the same hook `applyTheme()` uses, so both surfaces
// share one channel.
try {
  const t0 = performance.now();
  window.__themeLogEvt = (label, info) => {
    // `keepalive` so the POST survives a navigation away (pageshow breadcrumbs
    // in particular can fire just before a route swap). Relative URL (ADR 0014):
    // resolves against the engine-stamped `<base href="/<slug>/">`, or `/` at a
    // legacy root, so the breadcrumb reaches this workspace's engine through the
    // gateway with no inline prefix parsing.
    fetch('api/v1/internal/client-log', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        category: 'theme',
        message: label,
        data: Object.assign({ tMs: Math.round(performance.now() - t0) }, info),
      }),
      keepalive: true,
    }).catch(() => {});
  };
  window.__themeLogEvt('fouc', {
    raw: boot.raw,
    theme: boot.theme,
    resolved: boot.resolved,
    mqLight: boot.prefersLight,
  });
  // `pageshow` fires on a bfcache restore, when this script does NOT re-run, so
  // read live values here rather than trusting the stale entry above.
  window.addEventListener('pageshow', (e) => {
    window.__themeLogEvt?.('pageshow', {
      persisted: e.persisted,
      dataTheme: document.documentElement.getAttribute('data-theme'),
      rawNow: wsLocalGet('lucidos-theme'),
      mqLightNow: matchMedia('(prefers-color-scheme: light)').matches,
    });
  });
} catch {
  /* telemetry must never break FOUC */
}
