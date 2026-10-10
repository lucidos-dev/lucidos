import { showToast } from '../store';
import { cancelFormRequest } from '../../api/client/settings';
import { errorDetail } from '../../utils/errorDetail';

/** The Cancel of a credential or email form a *form request* opened. Closes
 *  the request on every device. A leaf module, so the form actions can call it
 *  without importing `form-requests.ts`, which imports them. */
export async function cancelFormRequestById(requestId: string): Promise<void> {
  try {
    await cancelFormRequest(requestId);
  } catch (e) {
    showToast(`Failed to cancel the request: ${errorDetail(e)}`, 'error');
  }
}
