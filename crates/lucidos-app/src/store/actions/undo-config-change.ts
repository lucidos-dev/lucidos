import type { Tap } from '@lucidos/sdk';
import { API } from '../../utils/basePath';
import { mutatingFetch, throwIfNotOk } from '../../api/client/_core';
import { errorDetail } from '../../utils/errorDetail';
import { showConfirm, showToast } from '../store';

const TITLE = 'Undo change';

/** Undo a config change on a user tap, after a confirmation. The engine
 * writes the inverse as the tapping device, and refuses one that moved on. */
export async function confirmConfigUndo(tap: Extract<Tap, { kind: 'undo-config-change' }>): Promise<void> {
  if (!(await showConfirm(
    'Undo this config change? A deleted credential comes back without its value, so someone must enter it again.',
    'Undo',
    { title: TITLE, variant: 'default' },
  ))) return;
  try {
    const response = await mutatingFetch(`${API}/config-changes/${encodeURIComponent(tap.event_id)}/undo`, {
      method: 'POST',
    });
    await throwIfNotOk(response);
    showToast('Change undone', 'success', { title: TITLE });
  } catch (error) {
    showToast(errorDetail(error), 'error', { title: TITLE });
  }
}
