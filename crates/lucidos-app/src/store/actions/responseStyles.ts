import { responseStyles, showToast } from '../store';
import { failedIfFresh, setLoadingIfFresh } from '../types';
import { listResponseStyles, setPreference } from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import type { StyleEntry } from '../../components/settings/responseStyle';
import type { ResponseStyle } from '../../api/types';

/** Load the merged *style library* into its signal.
 *
 *  A real read rather than a derivation of the cached preference map. The
 *  engine merges what it ships with what the user saved, and only the engine
 *  holds the shipped instructions. Refetches keep the old rows on screen
 *  through the round trip, the same flash-avoidance as `loadChatModels`. */
export async function loadResponseStyles(): Promise<void> {
  setLoadingIfFresh(responseStyles);
  try {
    const data = await listResponseStyles();
    responseStyles.value = { status: 'loaded', data: data.styles || [] };
  } catch (error) {
    // `failedIfFresh`, never `toFailed`: a failed REFETCH keeps the rows it
    // already has. The editor renders inside the loaded branch, so dropping
    // them mid-edit unmounts the form and takes the paragraph with it. Only a
    // first load with nothing to show becomes `failed`.
    responseStyles.value = failedIfFresh(responseStyles.value, error);
  }
}

/** Apply an edit to the CURRENT library and save the whole document.
 *
 *  `edit` is a function rather than a finished array, and that is the point.
 *  The key holds one document, so every save replaces all of it. Building that
 *  array from the library a form rendered minutes ago deletes whatever landed
 *  since: a style the agent added, or one a second device wrote. So the library
 *  is re-read first and the edit is applied to THAT.
 *
 *  It calls the API directly rather than going through `savePreference`, and
 *  that is deliberate too. The queued path applies the value locally, parks an
 *  undelivered write, and resolves either way. Right for a theme or a model id.
 *  Here the value is a document the engine bounds-checks, and a refusal has to
 *  keep the editor open with the paragraph the user typed.
 *
 *  Returns whether it landed. A refused write changes nothing on the engine,
 *  which validates before it writes, so the saved library is left as it was. */
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
    // A refusal throws too, carrying the engine's own reason. That reason is
    // what the editor needs. The form then stays open on the paragraph the
    // user typed, instead of closing over a lost edit.
    showToast(`The style was not saved: ${errorDetail(error)}`, 'error');
    return false;
  }
  await loadResponseStyles();
  return true;
}
