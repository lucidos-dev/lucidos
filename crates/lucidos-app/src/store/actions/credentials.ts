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
import {
  cancelPendingOAuthConnect,
  resumeOAuthConnectAfterCredentialSaved,
} from './oauth';
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

/** `id`, not the service name: a name no longer identifies one row. */
export function openEditCredential(id: string): void {
  landOnAccountsWithOverlay({ type: 'form', form: { type: 'credential', editing: id } });
  pushNavState();
}

export function openCredentialRequest(request: CredentialRequest): void {
  landOnAccountsWithOverlay({ type: 'form', form: { type: 'credential', request } });
  pushNavState();
}

export function closeCredentialForm(): void {
  // Dismissing the form abandons whatever Connect queued behind it. Without
  // this, cancelling a registration and later saving an unrelated credential
  // would open a browser for a provider the user walked away from.
  cancelPendingOAuthConnect();
  closeInlineForm();
}

/** Save a credential the engine ASKED for, then continue whatever was blocked.
 *
 *  Two things the plain create path cannot do, and the reason this exists as its
 *  own entry point rather than as flags on `submitNewCredential`:
 *
 *  1. **A repair updates, it never creates.** A request carrying
 *     `existing_credential_id` targets a row that already exists (an OAuth
 *     Client saved without the endpoints a flow needs). Creating would make a
 *     second `oauth_client` for one provider, and a name plus an auth type is
 *     the credential's identity, so that pair is a duplicate.
 *  2. **The flow continues.** Saving used to close the form and stop, which is
 *     what left a user who pressed Connect looking at the Accounts page with no
 *     idea that pressing Connect again was the next step.
 */
export async function submitRequestedCredential(
  request: CredentialRequest,
  service: string,
  baseUrls: string[],
  authType: AuthType,
  authValue: string,
  envVarName?: string,
): Promise<boolean> {
  const saved = request.existing_credential_id
    ? await submitCredentialEdit(request.existing_credential_id, {
        base_urls: baseUrls,
        auth_type: authType,
        // An `oauth_client` is not sent through the proxy auth pipeline, so it
        // has no meaningful auth header. The field is required by the update
        // shape, so it carries the same default the create path stores.
        auth_header: 'Authorization',
        auth_value: authValue,
        env_var_name: envVarName,
      })
    : await submitNewCredential(service, baseUrls, authType, authValue, envVarName);
  if (!saved) return false;
  await resumeOAuthConnectAfterCredentialSaved(service);
  return true;
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
    // `closeInlineForm`, NOT `closeCredentialForm`: closing because the save
    // SUCCEEDED must not abandon the authorization the save just unblocked.
    // Only a user dismissing the form does that.
    closeInlineForm();
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
  baseUrls: string[],
  authType: AuthType,
  authValue: string,
  envVarName?: string
): Promise<boolean> {
  if (!service) {
    showToast('Service name is required', 'error');
    return false;
  }
  // A `secret` is signed with rather than sent, so it declares no base URL and
  // an empty scope is its correct state. Requiring one here made the type
  // unsaveable from the credential form.
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
      }),
    'Failed to save credential'
  );
}

/** Update every editable field of an existing credential, addressed by `id`. */
export async function submitCredentialEdit(
  id: string,
  body: UpdateCredentialBody
): Promise<boolean> {
  return runCredentialSave(() => updateCredential(id, body), 'Failed to update credential');
}

/**
 * Delete by `id`. The service name is still passed, for the confirm prompt
 * only: "Delete credentials for \"google\"?" is what the user can act on, and
 * a uuid in a dialog is not.
 */
export async function deleteCredential(id: string, serviceName: string): Promise<void> {
  if (!(await showConfirm(`Delete credentials for "${serviceName}"?`))) {
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
