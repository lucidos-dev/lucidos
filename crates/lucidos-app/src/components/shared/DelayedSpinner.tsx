import { useDelayedFlag } from '../../hooks/useDelayedLoading';

/** Drop-in replacement for an immediately-rendered `<div class="loading-spinner" />`.
 *  Renders nothing for the first SPINNER_DELAY_MS after mount, so fast loads
 *  never flash a spinner. Mount it inside the branch that waits; when the wait
 *  subject changes identity without a remount (e.g. switching threads), key the
 *  parent so the delay restarts. */
export function DelayedSpinner() {
  return useDelayedFlag(true) ? <div class="loading-spinner" /> : null;
}
