import { effect } from '@preact/signals';
import { engineStartedAt, showToast } from '../store';
import { isTauri } from '../../utils/platform';
import { invoke } from '../../utils/tauri';

/** Tell the desktop shell when the engine this page shows started, so a
 *  development window can follow a switch to a new version (ADR 0461). The
 *  shell decides whether to re-exec onto a newer window binary, and a packaged
 *  window never does. Every start goes through, including the first, so a
 *  reload or a switch from another device reaches the same answer. Returns the
 *  teardown. */
export function installDevWindowFollow(): () => void {
  if (!isTauri()) return () => {};
  return effect(() => {
    const startedAt = engineStartedAt.value;
    if (!startedAt) return;
    const engineStartedAtMs = Date.parse(startedAt);
    if (Number.isNaN(engineStartedAtMs)) return;
    invoke<boolean>('follow_engine_switch', { engineStartedAtMs }).catch((e: unknown) => {
      showToast(`The dev window could not move to the new version: ${String(e)}`, 'error');
    });
  });
}
