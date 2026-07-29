import { environmentVariables, showToast, showConfirm } from '../store';
import { toFailed, setLoadingIfFresh } from '../types';
import {
  listEnvVars,
  createEnvVar,
  updateEnvVar,
  deleteEnvVarApi,
} from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';

export async function loadEnvironmentVariables(): Promise<void> {
  setLoadingIfFresh(environmentVariables);
  try {
    const data = await listEnvVars();
    environmentVariables.value = { status: 'loaded', data: data.env_vars || [] };
  } catch (error) {
    environmentVariables.value = toFailed(error);
  }
}

/** Create a new env var or update an existing one. Returns true on success. */
export async function submitEnvVar(
  name: string,
  value: string,
  isEdit: boolean
): Promise<boolean> {
  const trimmedName = name.trim();
  if (!trimmedName) {
    showToast('Name is required', 'error');
    return false;
  }
  try {
    const data = isEdit
      ? await updateEnvVar(trimmedName, { value })
      : await createEnvVar({ name: trimmedName, value });
    if (!data.success) {
      showToast(data.error || 'Failed to save environment variable', 'error');
      return false;
    }
    await loadEnvironmentVariables();
    return true;
  } catch (error) {
    showToast(`Failed to save environment variable: ${errorDetail(error)}`, 'error');
    return false;
  }
}

export async function deleteEnvironmentVariable(name: string): Promise<void> {
  if (!(await showConfirm(`Delete environment variable "${name}"?`, 'Delete'))) {
    return;
  }
  try {
    const data = await deleteEnvVarApi(name);
    if (data.success) {
      await loadEnvironmentVariables();
    } else {
      showToast(data.error || 'Failed to delete environment variable', 'error');
    }
  } catch (error) {
    showToast('Failed to delete environment variable: ' + errorDetail(error), 'error');
  }
}
