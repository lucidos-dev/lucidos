import { effect } from '@preact/signals';
import { startWorkspace } from '../../api/client/control';
import { errorDetail } from '../../utils/errorDetail';
import { connectionStatus, showToast } from '../store';
import { checkConnection, noteWorkspaceStartAccepted } from './connection';

/** Start the workspace this window belongs to, after the gateway reported it
 *  stopped. Resolves `true` once the engine answers, and `false` after telling
 *  the user why it did not.
 *
 *  The window reads `starting` from the moment the gateway accepts until then.
 *  The health poll owns when that ends (see `startStillBooting`). Probes at once
 *  rather than waiting for the 5s tick, so a fast boot lands without a pause. */
export async function startThisWorkspace(id: string): Promise<boolean> {
  try {
    await startWorkspace(id);
  } catch (err) {
    showToast(`Could not start the workspace: ${errorDetail(err)}`, 'error');
    return false;
  }
  noteWorkspaceStartAccepted();
  await checkConnection();
  if ((await leftStarting()) === 'connected') return true;
  showToast('The workspace did not start. Check the gateway log, then try again.', 'error');
  return false;
}

function leftStarting(): Promise<string> {
  return new Promise((resolve) => {
    const dispose = effect(() => {
      const status = connectionStatus.value;
      if (status === 'starting') return;
      queueMicrotask(() => dispose());
      resolve(status);
    });
  });
}
