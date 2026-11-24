
import { fixMainNow as apiFixMainNow, cancelLaneBatch as apiCancelLaneBatch, closeMainCheckRed, removeFromLandingLane as apiRemoveFromLandingLane } from '../../api/client/landingLane';
import { landingLaneCanceling, mainCheckClosing, showToast } from '../store';
import { errorDetail } from '../../utils/errorDetail';

export async function removeFromLandingLane(changeId: string): Promise<void> {
  try {
    await apiRemoveFromLandingLane(changeId);
  } catch (e) {
    showToast(errorDetail(e) || 'Failed to remove the change from the landing lane', 'error');
  }
}

export async function cancelLaneBatch(batchId: string): Promise<void> {
  landingLaneCanceling.value = batchId;
  try {
    await apiCancelLaneBatch(batchId);
  } catch (e) {
    landingLaneCanceling.value = null;
    showToast(errorDetail(e) || 'Failed to cancel the lane batch', 'error');
  }
}

export async function closeMainCheck(checkId: string): Promise<void> {
  mainCheckClosing.value = checkId;
  try {
    await closeMainCheckRed();
  } catch (e) {
    mainCheckClosing.value = null;
    showToast(errorDetail(e) || 'Failed to close the red main check', 'error');
  }
}

export async function fixMainNow(): Promise<void> {
  try {
    await apiFixMainNow();
  } catch (e) {
    showToast(errorDetail(e) || 'Failed to start the main repair', 'error');
  }
}
