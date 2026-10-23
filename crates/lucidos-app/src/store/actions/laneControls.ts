/**
 * The landing lane's Remove and Cancel, and the main check's Close: the
 * presses only the lazy lane surfaces offer (the Lucidos menu's lane row and
 * the Changes panel). The stream's half lives in `landingLane.ts` and
 * `mainCheck.ts`, which the first paint loads.
 */

import { cancelLaneBatch as apiCancelLaneBatch, closeMainCheckRed, removeFromLandingLane as apiRemoveFromLandingLane } from '../../api/client/landingLane';
import { landingLaneCanceling, mainCheckClosing, showToast } from '../store';
import { errorDetail } from '../../utils/errorDetail';

/** Remove a waiting change from the lane. The `ChangesUpdated` that follows
 *  carries the lane without it. */
export async function removeFromLandingLane(changeId: string): Promise<void> {
  try {
    await apiRemoveFromLandingLane(changeId);
  } catch (e) {
    showToast(errorDetail(e) || 'Failed to remove the change from the landing lane', 'error');
  }
}

/** Cancel the open lane batch. Its row drops Cancel until the batch closes,
 *  so a second press cannot fire. */
export async function cancelLaneBatch(batchId: string): Promise<void> {
  landingLaneCanceling.value = batchId;
  try {
    await apiCancelLaneBatch(batchId);
  } catch (e) {
    landingLaneCanceling.value = null;
    showToast(errorDetail(e) || 'Failed to cancel the lane batch', 'error');
  }
}

/** Close the red main check. Its line drops Close until the red closes, so a
 *  second press cannot fire. */
export async function closeMainCheck(checkId: string): Promise<void> {
  mainCheckClosing.value = checkId;
  try {
    await closeMainCheckRed();
  } catch (e) {
    mainCheckClosing.value = null;
    showToast(errorDetail(e) || 'Failed to close the red main check', 'error');
  }
}
