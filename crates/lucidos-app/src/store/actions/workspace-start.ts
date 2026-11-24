import { effect } from '@preact/signals';
import { startWorkspace } from '../../api/client/control';
import { errorDetail } from '../../utils/errorDetail';
import { connectionStatus, showToast } from '../store';
import { checkConnection, noteWorkspaceStartAccepted } from './connection';

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
