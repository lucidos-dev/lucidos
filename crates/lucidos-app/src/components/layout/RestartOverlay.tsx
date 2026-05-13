import { useEffect, useRef } from 'preact/hooks';
import { engineRestarting, showToast } from '../../store/store';

const RESTART_TIMEOUT_MS = 300_000;

export function RestartOverlay() {
  const active = engineRestarting.value;
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

    // Safety timeout — if the engine never comes back, unblock the UI
    const timer = setTimeout(() => {
      engineRestarting.value = false;
      showToast('Engine restart timed out', 'error');
    }, RESTART_TIMEOUT_MS);

    return () => {
      document.removeEventListener('keydown', block, true);
      document.removeEventListener('keyup', block, true);
      inerted.forEach((el) => el.removeAttribute('inert'));
      clearTimeout(timer);
    };
  }, [active]);

  if (!active) return null;

  return <div class="restart-overlay" ref={overlayRef} />;
}
