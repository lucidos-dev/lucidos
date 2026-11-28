import { effect } from '@preact/signals';
import { engineStartedAt, showToast } from '../store';
import { isTauri } from '../../utils/platform';
import { invoke } from '../../utils/tauri';

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
