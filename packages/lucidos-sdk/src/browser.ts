// IIFE bundle entry — adds iframe-only side effects to keep `index.ts` ES-import-safe for the frontend.
import { lucidos } from './index';
import { installScrollMemory } from './scroll';
import { installKeyboardForwarding } from './keyboardForward';
import { primeExternalLinkTarget } from './ui';

export * from './index';

if (typeof document !== 'undefined') {
  installScrollMemory();
  // Warm the external-link target so the click handler below can read it
  // synchronously (the "Ask" mode's share sheet needs the click's user
  // activation intact). Done here rather than leaning on applyPreferences,
  // which an app shipping its own visual identity never calls. Self-limits to
  // an installed iOS PWA, the only client where the value is read. Best-effort:
  // on failure links take the host path, exactly as they did before.
  primeExternalLinkTarget().catch((err) => {
    console.warn('[lucidos-sdk] could not read the external-link preference:', err);
  });
  // Forward host keyboard shortcuts (pane focus/hide, narrow/widen, Escape, …)
  // up to the parent — iframe keydowns never reach the host document otherwise,
  // so shortcuts die whenever an app has focus. See keyboardForward.ts.
  installKeyboardForwarding();

  document.addEventListener('click', (e: MouseEvent) => {
    const target = e.target as Element | null;
    const anchor = target?.closest?.('a[href]') as HTMLAnchorElement | null;
    if (!anchor) return;
    const href = anchor.getAttribute('href');
    if (!href) return;
    if (/^https?:\/\//.test(href)) {
      e.preventDefault();
      e.stopPropagation();
      // openExternal, not navigate: it honours the user's external-link
      // preference and, in "Ask" mode, must run inside this frame while the
      // click's user activation is still live. Called synchronously here for
      // exactly that reason.
      //
      // The window.open fallback stays. It only fires once the host route has
      // already failed (engine unreachable), and in that state the preference
      // cannot be honoured by any path, so the choice is between showing the
      // page in the in-app web view and the tap doing nothing at all. A visible
      // page wins: the preference is about where a link PREFERS to open, not a
      // reason to swallow it when nothing else is left.
      lucidos.ui.openExternal(href).catch((err) => {
        console.warn('[lucidos-sdk] external link fell back to window.open:', href, err);
        window.open(href, '_blank', 'noopener');
      });
    } else if (anchor.getAttribute('target') === '_blank') {
      e.preventDefault();
      e.stopPropagation();
      const resolved = new URL(href, window.location.href).pathname;
      window.location.href = resolved;
    }
  }, true);
}
