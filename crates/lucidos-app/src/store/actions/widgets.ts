
import { listReusableWidgetsApi } from '../../api/client/widgets';
import { loadingIfFresh, toFailed } from '../types';
import { reusableWidgets } from '../widgets';

export function refreshReusableWidgets(): void {
  if (reusableWidgets.value.status !== 'not-loaded') void loadReusableWidgets();
}

export async function loadReusableWidgets(): Promise<void> {
  reusableWidgets.value = loadingIfFresh(reusableWidgets.value);
  try {
    reusableWidgets.value = { status: 'loaded', data: await listReusableWidgetsApi() };
  } catch (error) {
    if (reusableWidgets.value.status !== 'loaded') reusableWidgets.value = toFailed(error);
  }
}
