/** The reusable widgets list, and how SSE keeps it fresh (ADR 0402). What
 *  the user does to a widget lives in `widget-actions.ts`, so the SSE path
 *  never loads the menus' imports into the entry chunk. */

import { listReusableWidgetsApi } from '../../api/client/widgets';
import { loadingIfFresh, toFailed } from '../types';
import { reusableWidgets } from '../widgets';

/** An app changed or went away: re-read the reusable list, which a widget
 *  may have just joined or left. */
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
