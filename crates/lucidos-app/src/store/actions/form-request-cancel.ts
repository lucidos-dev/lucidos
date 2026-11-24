import { showToast } from '../store';
import { cancelFormRequest } from '../../api/client/settings';
import { errorDetail } from '../../utils/errorDetail';

export async function cancelFormRequestById(requestId: string): Promise<void> {
  try {
    await cancelFormRequest(requestId);
  } catch (e) {
    showToast(`Failed to cancel the request: ${errorDetail(e)}`, 'error');
  }
}
