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
// Set by the inline handover script in index.html when the gateway boot splash
// had already built the mark on this url, so this document skips its reveal.
const FORMED_CLASS_SELECTOR = '.boot-splash-formed';
// Set by the inline quiet-cover script in index.html on a document whose load
// CONTINUES a session rather than starting one, which carries no mark at all.
// Two triggers: a user-requested refresh (permanent) and a notification-tap
// deep link (whose URL form is the temporary half, tracked at
// docs/temporary-measures.md § "Cross-document notification-tap reload on iOS").
// The cover itself is permanent either way: do NOT delete it with that measure.
const QUIET_CLASS_SELECTOR = '.boot-splash-quiet';

// Matches the longest `.boot-splash-leaving` fade in index.html (0.45s); the
// extra margin covers the reduced-motion 0.15s case too. Used as the removal
// fallback when `animationend` doesn't fire.
const FADE_REMOVE_MS = 550;

let dismissed = false;

/** True while the inline splash is still in the DOM. */
export function bootSplashPresent(): boolean {
  return !dismissed && document.querySelector(SPLASH_SELECTOR) !== null;
}

/** True when this document plays no mark reveal, so `useBootSplashReady` has no
 *  min-reveal floor to hold before dismissing. Two documents qualify, both
 *  decided by an inline script in index.html before first paint:
 *
 *  - `boot-splash-formed` (gateway handover): the mark was already built by the
 *    gateway boot splash on this url and is standing on screen, so this document
 *    only carries it.
 *  - `boot-splash-quiet` (a refresh, or a notification tap): the document
 *    carries no mark at all. See QUIET_CLASS_SELECTOR above.
 *
 *  Both arms are permanent.
 *
 *  One predicate rather than two exported halves, because the caller only ever
 *  wants the disjunction, and a caller-side `a() || b()` is what a unit test
 *  cannot reach through the module mock. */
export function bootSplashPlaysNoReveal(): boolean {
  return (
    document.querySelector(SPLASH_SELECTOR + FORMED_CLASS_SELECTOR) !== null ||
    document.querySelector(SPLASH_SELECTOR + QUIET_CLASS_SELECTOR) !== null
  );
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

/** Reveal the inline splash's gateway escape link, when this document has one to
 *  offer. Returns true if a link is now showing.
 *
 *  The link, its href rule and the conditions for offering it all live in the
 *  inline document (see the boot watchdog in index.html), which exposes the
 *  reveal as `window.__lucidosGatewayEscape`. This is a thin call through rather
 *  than a second implementation: the same escape must be offered whether or not
 *  the application bundle loaded, and two copies of "where does the gateway live"
 *  would be free to drift apart.
 *
 *  Returns false when the hook is absent (the splash is already gone, or this
 *  document is behind the gateway / has no gateway to escape to) so callers can
 *  stay indifferent to which case they are in. */
export function revealBootEscape(): boolean {
  const reveal = (window as Window & { __lucidosGatewayEscape?: () => unknown })
    .__lucidosGatewayEscape;
  if (typeof reveal !== 'function') return false;
  return reveal() != null;
}

/** Fade out and remove the splash. Idempotent — safe to call from the readiness
 *  gate, the safety cap, and the picker's "show the list" path. */
export function dismissBootSplash(): void {
  if (dismissed) return;
  dismissed = true;
  // Revert the html + body backgrounds the boot document painted with the brand
  // gradient for iOS safe-area coverage, so the app shell's own
  // var(--bg-primary) backgrounds show once the splash is gone — otherwise the
  // blue gradient lingers behind
  // the app's bottom safe-area inset. Done ONLY after the splash node is
  // actually removed: reverting during the `.boot-splash-leaving` fade would
  // expose the uncovered bottom safe-area strip (reverting it to dark
  // --bg-primary) while the splash is still visibly fading, briefly flashing
  // the very black band this whole change removes.
  const revertDocumentBackground = () => {
    if (typeof document !== 'undefined' && document.documentElement) {
      document.documentElement.style.background = '';
      if (document.body) document.body.style.background = '';
    }
  };
  const el = document.querySelector(SPLASH_SELECTOR);
  if (!el) {
    revertDocumentBackground();
    return;
  }
  el.classList.add(LEAVING_CLASS);
  let removed = false;
  const remove = () => {
    if (removed) return;
    removed = true;
    el.remove();
    revertDocumentBackground();
  };
  el.addEventListener('animationend', remove, { once: true });
  // Fallback: if the fade animation is suppressed (no animationend), still remove.
  window.setTimeout(remove, FADE_REMOVE_MS);
}
