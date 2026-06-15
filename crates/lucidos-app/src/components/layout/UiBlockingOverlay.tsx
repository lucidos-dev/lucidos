import { useEffect, useRef } from 'preact/hooks';
import { engineRestarting } from '../../store/store';
import { clientRefreshing } from '../../hooks/sw-update';
import { handleRestartTimeout } from '../../store/actions/connection';

const RESTART_TIMEOUT_MS = 300_000;

/** Full-screen blocker shown while the UI must be locked: during an engine
 *  restart (`engineRestarting`) or a client refresh (`clientRefreshing`). Both
 *  cover the screen, drop focus, swallow keystrokes, and mark every sibling
 *  inert so no click, focus, or input lands mid-transition. The toast container
 *  stays interactive so the restart status toast remains visible/dismissible. */
export function UiBlockingOverlay() {
  const active = engineRestarting.value || clientRefreshing.value;
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

    // Safety timeout — restart only. If the engine never comes back, unblock the
    // UI; handleRestartTimeout probes health before declaring a timeout so a
    // frozen timer firing on iOS PWA resume (engine already restarted) doesn't
    // show a false error — see its doc comment in connection.ts. A refresh needs
    // no fallback: it always ends in a page reload that tears the overlay down.
    const timer = engineRestarting.value
      ? setTimeout(() => { void handleRestartTimeout(); }, RESTART_TIMEOUT_MS)
      : null;

    return () => {
      document.removeEventListener('keydown', block, true);
      document.removeEventListener('keyup', block, true);
      inerted.forEach((el) => el.removeAttribute('inert'));
      document.documentElement.removeAttribute('data-ui-blocked');
      if (timer) clearTimeout(timer);
    };
  }, [active]);

  if (!active) return null;

  return <div class="ui-blocking-overlay" ref={overlayRef} />;
}
