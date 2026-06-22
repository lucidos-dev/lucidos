/**
 * Boot-splash controller.
 *
 * The splash itself is inlined in `index.html` (markup + style) so it paints on
 * the FIRST frame, before this bundle loads — that is what makes the cold launch
 * connection-independent. This module is the runtime that takes over once the
 * bundle is live: it updates the status line and dismisses the splash when the
 * app is ready (readiness-gated — see `hooks/useBootSplashReady.ts`) or when the
 * picker decides to show its workspace list.
 *
 * It operates on the inline DOM via `querySelector` (the `#app`-only
 * getElementById ban in `.claude/rules/frontend.md`); the splash node is a
 * sibling of `#app` in `<body>`, owned by no Preact tree, so manual teardown
 * here never fights reconciliation.
 *
 * Across the picker→workspace cross-document navigation each document carries its
 * own inline splash, so module state does not need to persist — a fresh document
 * gets a fresh, present splash.
 */

const SPLASH_SELECTOR = '.boot-splash';
const STATUS_SELECTOR = '.boot-splash-status';
const LEAVING_CLASS = 'boot-splash-leaving';

// Matches the longest `.boot-splash-leaving` fade in index.html (0.45s); the
// extra margin covers the reduced-motion 0.15s case too. Used as the removal
// fallback when `animationend` doesn't fire.
const FADE_REMOVE_MS = 550;

let dismissed = false;

/** True while the inline splash is still in the DOM. */
export function bootSplashPresent(): boolean {
  return !dismissed && document.querySelector(SPLASH_SELECTOR) !== null;
}

/** Update the status line under the mark (e.g. "Opening your workspace…",
 *  "Connecting…") and fade it in (it starts hidden so a fast load never flashes
 *  text). No-op if the splash is absent. */
export function setBootStatus(text: string): void {
  const el = document.querySelector(SPLASH_SELECTOR + ' ' + STATUS_SELECTOR);
  if (!el) return;
  el.textContent = text;
  el.classList.toggle('boot-splash-status-shown', text.length > 0);
}

/** Fade out and remove the splash. Idempotent — safe to call from the readiness
 *  gate, the safety cap, and the picker's "show the list" path. */
export function dismissBootSplash(): void {
  if (dismissed) return;
  dismissed = true;
  const el = document.querySelector(SPLASH_SELECTOR);
  if (!el) return;
  el.classList.add(LEAVING_CLASS);
  let removed = false;
  const remove = () => {
    if (removed) return;
    removed = true;
    el.remove();
  };
  el.addEventListener('animationend', remove, { once: true });
  // Fallback: if the fade animation is suppressed (no animationend), still remove.
  window.setTimeout(remove, FADE_REMOVE_MS);
}
