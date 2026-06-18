import {
  credentials,
  closeInlineForm,
  showToast,
  showConfirm,
} from '../store';
import { toFailed, setLoadingIfFresh } from '../types';
import type { AuthType, CredentialRequest } from '../types';
import {
  listCredentials,
  createCredential,
  updateCredential,
  deleteCredentialApi,
} from '../../api/client';
import type { ApiResult } from '../../api/types';
import type { UpdateCredentialBody } from '../../api/client/settings';
import { landOnAccountsWithOverlay } from './menu';
import { pushNavState } from './navigation';
import { errorDetail } from '../../utils/errorDetail';

export async function loadCredentials(): Promise<void> {
  setLoadingIfFresh(credentials);
  try {
    const data = await listCredentials();
    credentials.value = { status: 'loaded', data: data.credentials || [] };
  } catch (error) {
    credentials.value = toFailed(error);
  }
}

export function openAddCredential(): void {
  landOnAccountsWithOverlay({ type: 'form', form: { type: 'credential' } });
  pushNavState();
}

export function openEditCredential(serviceName: string): void {
  landOnAccountsWithOverlay({ type: 'form', form: { type: 'credential', editing: serviceName } });
  pushNavState();
}

export function openCredentialRequest(request: CredentialRequest): void {
  landOnAccountsWithOverlay({ type: 'form', form: { type: 'credential', request } });
  pushNavState();
}

export function closeCredentialForm(): void {
  closeInlineForm();
}

/** Shared success/error/reload handling for credential saves. */
async function runCredentialSave(
  apiCall: () => Promise<ApiResult>,
  failMsg: string
): Promise<boolean> {
  try {
    const data = await apiCall();
    if (!data.success) {
      showToast(data.error || failMsg, 'error');
      return false;
    }
    closeCredentialForm();
    await loadCredentials();
    return true;
  } catch (error) {
    showToast(`${failMsg}: ${errorDetail(error)}`, 'error');
    return false;
  }
}

/** Create a brand-new credential (also used by the engine credential-request flow). */
export async function submitNewCredential(
  service: string,
  baseUrl: string,
  authType: AuthType,
  authValue: string,
  envVarName?: string
): Promise<boolean> {
  if (!service || !baseUrl) {
    showToast('Service name and base URL are required', 'error');
    return false;
  }
  if (!authValue) {
    showToast('Token/API key is required', 'error');
    return false;
  }
  return runCredentialSave(
    () =>
      createCredential({
        service_name: service,
        base_url: baseUrl,
        auth_type: authType,
        auth_value: authValue,
        env_var_name: envVarName?.trim() || undefined,
      }),
    'Failed to save credential'
  );
}

/** Update every editable field of an existing credential. */
export async function submitCredentialEdit(
  service: string,
  body: UpdateCredentialBody
): Promise<boolean> {
  return runCredentialSave(() => updateCredential(service, body), 'Failed to update credential');
}

export async function deleteCredential(serviceName: string): Promise<void> {
  if (!(await showConfirm(`Delete credentials for "${serviceName}"?`))) {
    return;
  }

  try {
    const data = await deleteCredentialApi(serviceName);
    if (data.success) {
      await loadCredentials();
    } else {
      showToast(data.error || 'Failed to delete credential', 'error');
    }
  } catch (error) {
    showToast('Failed to delete credential: ' + errorDetail(error), 'error');
  }
}
