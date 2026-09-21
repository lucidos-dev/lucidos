// IIFE bundle entry — adds iframe-only side effects to keep `index.ts` ES-import-safe for the frontend.
import { lucidos } from './index';
import { installScrollMemory } from './scroll';
import { installKeyboardForwarding } from './keyboardForward';
import { installTooltips } from './tooltip';
import { primeExternalLinkTarget } from './ui';
import { isBridged } from './_bridge';
import { primeBridgedStorage } from './_storage';
import { installHostOps } from './hostOps';
import { installCapabilityRenewal } from './frameCapability';

export * from './index';

if (typeof document !== 'undefined') {
  // Answer the host's own requests: capture, app switch, fragment delivery. It
  // reached through `contentWindow` for all three until an opaque origin closed
  // that. Installed unconditionally, so the host can ask the same way whichever
  // frame it got.
  installHostOps();
  // Take the renewed pass the host pushes before this one lapses (ADR 0238).
  // The engine seeded the first one into `<base href>`, and each renewal is one
  // attribute write. Nothing reloads, and an already-loaded resource is
  // untouched. A document with no pass ignores every push.
  installCapabilityRenewal();
  // Scroll memory reads storage, and an isolated frame's own storage throws, so
  // the host holds it. That read is async and the restore is not, so the values
  // are fetched first. `primeBridgedStorage` is a no-op in a frame that can read
  // for itself, which is why the direct path still installs synchronously.
  if (isBridged()) {
    primeBridgedStorage()
      .catch((err) => {
        console.warn('[lucidos-sdk] could not read this app\'s stored values:', err);
      })
      // Either way: an empty mirror reads as a first visit, which is a position
      // worth landing at rather than no scroll memory at all from here on.
      .finally(() => { installScrollMemory(); });
  } else {
    installScrollMemory();
  }
  // A themed tooltip on any data-tooltip element, with no init call from the
  // app. The two options differ from the host shell, because an app author
  // never wires a touch affordance by hand: any data-tooltip answers a long
  // press, and the tooltip clears itself shortly after the finger lifts.
  installTooltips({ longPressNeedsOptIn: false, hideAfterLongPressMs: 2000 });
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
    // A download of one of the app's OWN files. The frame is isolated, so that
    // file is cross-origin to it and the browser ignores the `download`
    // attribute: the click would navigate the frame to the file. Ask the engine
    // for it as an attachment instead, which is what a browser does obey.
    // `blob:` and `data:` need none of this and are left alone.
    if (isBridged() && anchor.hasAttribute('download') && !/^[a-z][a-z0-9+.-]*:/i.test(href)) {
      e.preventDefault();
      e.stopPropagation();
      // Against `document.baseURI`, which is what the browser itself resolves
      // this href against. Behind a gateway the base carries the frame
      // capability (ADR 0238). `window.location.href` does not, so the older
      // form asked for a file with no pass on it.
      const url = new URL(href, document.baseURI);
      url.searchParams.set('download', '1');
      const via = document.createElement('a');
      via.href = url.href;
      via.download = anchor.getAttribute('download') || '';
      via.rel = 'noopener';
      document.body.appendChild(via);
      via.click();
      // Removed on a later task, never in the same one. A download starts
      // asynchronously, and tearing the anchor out from under it is enough to
      // lose it on some engines.
      setTimeout(() => via.remove(), 0);
      return;
    }
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
