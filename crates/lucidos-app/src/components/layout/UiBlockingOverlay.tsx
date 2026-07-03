import { useEffect, useRef } from 'preact/hooks';
import { clientRefreshing } from '../../hooks/sw-update';

/** Full-screen blocker shown while a client refresh (`clientRefreshing`) swaps
 *  the service worker and reloads: it covers the screen, drops focus, swallows
 *  keystrokes, and marks every sibling inert so no click, focus, or input lands
 *  mid-reload. The toast container stays interactive so the "Refreshing…" status
 *  toast remains visible.
 *
 *  Engine restart deliberately does NOT block here: the gateway boot splash, the
 *  GET-gate (`awaitEngineReady`), and SSE auto-reconnect make a restart a
 *  recoverable non-event, so the initiating tab stays interactive and only shows
 *  a light dismissible "Restarting engine…" toast. The restart safety-timeout
 *  that used to live in this effect now rides `engineRestarting` directly in
 *  `store/effects.ts`. */
export function UiBlockingOverlay() {
  const active = clientRefreshing.value;
  const overlayRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!active) return;
    const overlay = overlayRef.current;
    if (!overlay) return;

    const block = (e: Event) => {
      e.stopPropagation();
      e.preventDefault();
    };

    // Capture phase — catches global shortcut handlers on document/window
    // that inert can't reach.
    document.addEventListener('keydown', block, true);
    document.addEventListener('keyup', block, true);

    // Drop focus so any in-flight typing or IME composition stops.
    (document.activeElement as HTMLElement | null)?.blur();

    // Flag the block globally so CSS can keep the one element that outranks
    // this overlay — the JS tooltip (#tooltip at --z-tooltip = 10000, kept that
    // high so a modal never clips it) — from floating above the dim blocker.
    // Only the toast container is allowed above the overlay.
    document.documentElement.setAttribute('data-ui-blocked', '');

    // Mark every sibling of the overlay inert so panels, drawers, and modals
    // stop receiving clicks, focus, and input. The toast container stays
    // interactive so the user can dismiss the restart toast.
    const inerted: HTMLElement[] = [];
    const root = overlay.parentElement;
    if (root) {
      for (const child of Array.from(root.children) as HTMLElement[]) {
        if (child === overlay) continue;
        if (child.classList.contains('toast-container')) continue;
        if (child.hasAttribute('inert')) continue;
        child.setAttribute('inert', '');
        inerted.push(child);
      }
    }

    // No safety timeout here — a client refresh always ends in a page reload that
    // tears this overlay down. (The engine-restart safety timeout lives in
    // store/effects.ts, keyed on `engineRestarting`.)

    return () => {
      document.removeEventListener('keydown', block, true);
      document.removeEventListener('keyup', block, true);
      inerted.forEach((el) => el.removeAttribute('inert'));
      document.documentElement.removeAttribute('data-ui-blocked');
    };
  }, [active]);

  if (!active) return null;

  return <div class="ui-blocking-overlay" ref={overlayRef} />;
}
