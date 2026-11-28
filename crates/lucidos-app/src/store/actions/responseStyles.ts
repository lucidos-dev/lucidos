import { responseStyles, showToast } from '../store';
import { failedIfFresh, setLoadingIfFresh } from '../types';
import { listResponseStyles, setPreference } from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import type { StyleEntry } from '../../components/settings/responseStyle';
import type { ResponseStyle } from '../../api/types';

export async function loadResponseStyles(): Promise<void> {
  setLoadingIfFresh(responseStyles);
  try {
    const data = await listResponseStyles();
    responseStyles.value = { status: 'loaded', data: data.styles || [] };
  } catch (error) {
    responseStyles.value = failedIfFresh(responseStyles.value, error);
  }
}

export async function saveStyleDocument(
  edit: (library: readonly ResponseStyle[]) => StyleEntry[],
): Promise<boolean> {
  await loadResponseStyles();
  const current = responseStyles.value;
  if (current.status !== 'loaded') {
    showToast('The style was not saved: could not read the current styles', 'error');
    return false;
  }

  try {
    await setPreference('response_styles', JSON.stringify(edit(current.data)));
  } catch (error) {
    showToast(`The style was not saved: ${errorDetail(error)}`, 'error');
    return false;
  }
  await loadResponseStyles();
  return true;
}
