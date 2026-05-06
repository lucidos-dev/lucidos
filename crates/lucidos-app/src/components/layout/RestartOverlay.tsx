import { useEffect } from 'preact/hooks';
import { engineRestarting, showToast } from '../../store/store';

const RESTART_TIMEOUT_MS = 300_000;

export function RestartOverlay() {
  const active = engineRestarting.value;

  useEffect(() => {
    if (!active) return;

    const block = (e: Event) => {
      e.stopPropagation();
      e.preventDefault();
    };

    // Capture phase — intercepts before any handler sees the event
    document.addEventListener('keydown', block, true);
    document.addEventListener('keyup', block, true);

    // Safety timeout — if the engine never comes back, unblock the UI
    const timer = setTimeout(() => {
      engineRestarting.value = false;
      showToast('Engine restart timed out', 'error');
    }, RESTART_TIMEOUT_MS);

    return () => {
      document.removeEventListener('keydown', block, true);
      document.removeEventListener('keyup', block, true);
      clearTimeout(timer);
    };
  }, [active]);

  if (!active) return null;

  return (
    <div class="restart-overlay" />
  );
}
