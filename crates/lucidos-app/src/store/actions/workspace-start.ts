import { effect } from '@preact/signals';
import { startWorkspace } from '../../api/client/control';
import { errorDetail } from '../../utils/errorDetail';
import { connectionStatus, showToast } from '../store';
import { checkConnection } from './connection';

/** How long an accepted start may leave the workspace `stopped`. The gateway
 *  boots in the background, so a boot that fails there shows only as a
 *  workspace that never leaves `stopped`. Several health ticks fit inside. */
export const START_SETTLE_MS = 20_000;

/** Start the workspace this window belongs to, after the gateway reported it
 *  stopped. Resolves `true` once the workspace has left `stopped`, and `false`
 *  after telling the user why it did not.
 *
 *  Probes health on success rather than waiting for the 5s tick, so the bar
 *  moves on as soon as the engine is booting. */
export async function startThisWorkspace(id: string): Promise<boolean> {
  try {
    await startWorkspace(id);
  } catch (err) {
    showToast(`Could not start the workspace: ${errorDetail(err)}`, 'error');
    return false;
  }
  await checkConnection();
  if (await leftStopped()) return true;
  showToast('The workspace did not start. Check the gateway log, then try again.', 'error');
  return false;
}

function leftStopped(): Promise<boolean> {
  return new Promise((resolve) => {
    let dispose = () => {};
    const timer = setTimeout(() => {
      dispose();
      resolve(false);
    }, START_SETTLE_MS);
    dispose = effect(() => {
      if (connectionStatus.value === 'stopped') return;
      clearTimeout(timer);
      queueMicrotask(() => dispose());
      resolve(true);
    });
  });
}
