import { updateAvailable, showToast, removeToast, dismissToast } from '../store';
import { preferencesSettled } from './preferences';
import {
  getServedBuildId,
  refreshClient,
  wasSwUpdateDismissed,
  noteUpdateBuildId,
} from '../../hooks/sw-update';
import { isUnstampedBuildId } from '../../utils/buildId';
import { CLIENT_BUILD_ID } from 'virtual:build-id';

const UPDATE_TOAST_KEY = 'update-available';

function surfaceUpdateToast(servedBuildId: string): void {
  noteUpdateBuildId(servedBuildId);
  showToast('New version available — refresh to sync', 'info', {
    key: UPDATE_TOAST_KEY,
    secondaryAction: {
      label: 'Later',
      onClick: () => { dismissToast(UPDATE_TOAST_KEY); },
    },
    action: {
      label: 'Refresh',
      onClick: () => {
        removeToast(UPDATE_TOAST_KEY);
        refreshClient();
      },
    },
  });
}

export async function syncClientUpdateFromBuild(): Promise<boolean> {
  if (isUnstampedBuildId(CLIENT_BUILD_ID)) return false;
  if (!preferencesSettled.value) return false;
  const served = await getServedBuildId();
  if (served === null) return false;
  const stale = served !== CLIENT_BUILD_ID;
  updateAvailable.value = stale;
  if (stale && !wasSwUpdateDismissed(served)) {
    surfaceUpdateToast(served);
  } else {
    removeToast(UPDATE_TOAST_KEY);
  }
  return stale;
}
