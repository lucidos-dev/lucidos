import {
  activeInlineForm,
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
import {
  cancelPendingOAuthConnect,
  resumeOAuthConnectAfterCredentialSaved,
} from './oauth';
import { pushNavState } from './navigation';
import { cancelFormRequestById } from './form-request-cancel';
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

export function openEditCredential(id: string): void {
  landOnAccountsWithOverlay({ type: 'form', form: { type: 'credential', editing: id } });
  pushNavState();
}

export function openCredentialRequest(request: CredentialRequest): void {
  landOnAccountsWithOverlay({ type: 'form', form: { type: 'credential', request } });
  pushNavState();
}

export function closeCredentialForm(): void {
  cancelPendingOAuthConnect();
  const form = activeInlineForm.value;
  const requestId = form?.type === 'credential' ? form.request?.form_request_id : undefined;
  closeInlineForm();
  if (requestId) void cancelFormRequestById(requestId);
}

export async function submitRequestedCredential(
  request: CredentialRequest,
  service: string,
  baseUrls: string[],
  authType: AuthType,
  authValue: string,
  envVarName?: string,
  authHeader?: string,
): Promise<boolean> {
  const saved = request.existing_credential_id
    ? await submitCredentialEdit(request.existing_credential_id, {
        base_urls: baseUrls,
        auth_type: authType,
        auth_header: authHeader?.trim() || 'Authorization',
        auth_value: authValue,
        env_var_name: envVarName,
        form_request_id: request.form_request_id,
      })
    : await submitNewCredential(
        service,
        baseUrls,
        authType,
        authValue,
        envVarName,
        request.form_request_id,
      );
  if (!saved) return false;
  await resumeOAuthConnectAfterCredentialSaved(service);
  return true;
}

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
    closeInlineForm();
    await loadCredentials();
    return true;
  } catch (error) {
    showToast(`${failMsg}: ${errorDetail(error)}`, 'error');
    return false;
  }
}

export async function submitNewCredential(
  service: string,
  baseUrls: string[],
  authType: AuthType,
  authValue: string,
  envVarName?: string,
  formRequestId?: string,
): Promise<boolean> {
  if (!service) {
    showToast('Service name is required', 'error');
    return false;
  }
  if (authType !== 'secret' && baseUrls.length === 0) {
    showToast('Base URL is required', 'error');
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
        base_urls: baseUrls,
        auth_type: authType,
        auth_value: authValue,
        env_var_name: envVarName?.trim() || undefined,
        form_request_id: formRequestId,
      }),
    'Failed to save credential'
  );
}

export async function submitCredentialEdit(
  id: string,
  body: UpdateCredentialBody
): Promise<boolean> {
  return runCredentialSave(() => updateCredential(id, body), 'Failed to update credential');
}

export async function deleteCredential(id: string, serviceName: string): Promise<void> {
  if (!(await showConfirm(`Delete credentials for "${serviceName}"?`, 'Delete', { variant: 'danger' }))) {
    return;
  }

  try {
    const data = await deleteCredentialApi(id);
    if (data.success) {
      await loadCredentials();
    } else {
      showToast(data.error || 'Failed to delete credential', 'error');
    }
  } catch (error) {
    showToast('Failed to delete credential: ' + errorDetail(error), 'error');
  }
}
