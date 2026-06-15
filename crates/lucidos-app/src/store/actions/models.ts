import { chatModels, showToast, showConfirm } from '../store';
import { toFailed, setLoadingIfFresh } from '../types';
import { MODELS } from '../models';
import { listModels, createModel, updateModel, deleteModelApi } from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';

/** Load the DB-backed model registry into the `chatModels` signal. Refetches
 *  (e.g. after a Model* SSE) keep showing existing data through the round trip
 *  and swap atomically — same flash-avoidance as `loadCredentials`. */
export async function loadChatModels(): Promise<void> {
  setLoadingIfFresh(chatModels);
  try {
    const data = await listModels();
    chatModels.value = { status: 'loaded', data: data.models || [] };
  } catch (error) {
    chatModels.value = toFailed(error);
  }
}

/** Options for the chat model `<Dropdown>` — enabled models from the loaded
 *  registry, falling back to the static `MODELS` list before the first load (so
 *  the picker never renders empty). */
export function chatModelOptions(): Array<{ value: string; label: string }> {
  const loadable = chatModels.value;
  if (loadable.status === 'loaded') {
    return loadable.data
      .filter((m) => m.enabled)
      .map((m) => ({ value: m.id, label: m.label }));
  }
  return MODELS;
}

/** Shared success/error/reload handling for model mutations, mirroring
 *  `runCredentialSave`. Returns whether the mutation succeeded. */
async function runModelMutation(
  apiCall: () => Promise<{ success: boolean; error?: string }>,
  failMsg: string
): Promise<boolean> {
  try {
    const data = await apiCall();
    if (!data.success) {
      showToast(data.error || failMsg, 'error');
      return false;
    }
    await loadChatModels();
    return true;
  } catch (error) {
    showToast(`${failMsg}: ${errorDetail(error)}`, 'error');
    return false;
  }
}

export async function submitNewModel(
  id: string,
  label: string,
  provider: string
): Promise<boolean> {
  if (!id.trim() || !label.trim()) {
    showToast('Model id and label are required', 'error');
    return false;
  }
  return runModelMutation(
    () => createModel({ id: id.trim(), label: label.trim(), provider }),
    'Failed to add model'
  );
}

export function setModelEnabled(id: string, enabled: boolean): Promise<boolean> {
  return runModelMutation(() => updateModel(id, { enabled }), 'Failed to update model');
}

export async function deleteModel(id: string): Promise<void> {
  if (!(await showConfirm(`Delete model "${id}"?`, 'Delete'))) {
    return;
  }
  await runModelMutation(() => deleteModelApi(id), 'Failed to delete model');
}
