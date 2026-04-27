import {
  credentials,
  activeInlineForm,
  panelOverlay,
  closeInlineForm,
  showToast,
  showConfirm,
} from '../store';
import { toFailed } from '../types';
import type { AuthType, CredentialRequest } from '../types';
import {
  listCredentials,
  createCredential,
  updateCredential,
  deleteCredentialApi,
} from '../../api/client';
import { navigateToAccounts } from './menu';
import { pushNavState } from './navigation';
import { errorDetail } from '../../utils/errorDetail';

export async function loadCredentials(): Promise<void> {
  if (credentials.value.status !== 'loaded') {
    credentials.value = { status: 'loading' };
  }
  try {
    const data = await listCredentials();
    credentials.value = { status: 'loaded', data: data.credentials || [] };
  } catch (error) {
    console.error('Failed to load credentials:', error);
    credentials.value = toFailed(error);
  }
}

export function openAddCredential(): void {
  navigateToAccounts();
  panelOverlay.value = { type: 'form', form: { type: 'credential' } };
  pushNavState();
}

export function openEditCredential(serviceName: string): void {
  navigateToAccounts();
  panelOverlay.value = { type: 'form', form: { type: 'credential', editing: serviceName } };
  pushNavState();
}

export function openCredentialRequest(request: CredentialRequest): void {
  navigateToAccounts();
  panelOverlay.value = { type: 'form', form: { type: 'credential', request } };
  pushNavState();
}

export function closeCredentialForm(): void {
  closeInlineForm();
}

export async function submitCredential(
  service: string,
  baseUrl: string,
  authType: AuthType,
  authValue: string
): Promise<boolean> {
  if (!service || !baseUrl) {
    showToast('Service name and base URL are required', 'error');
    return false;
  }

  const form = activeInlineForm.value;
  const editing = form?.type === 'credential' ? form.editing : undefined;

  if (!editing && !authValue) {
    showToast('Token/API key is required', 'error');
    return false;
  }

  try {
    if (editing && authValue) {
      const data = await updateCredential(service, authValue);
      if (!data.success) {
        showToast(data.error || 'Failed to update credential', 'error');
        return false;
      }
    } else if (!editing) {
      const data = await createCredential({
        service_name: service,
        base_url: baseUrl,
        auth_type: authType,
        auth_value: authValue,
      });
      if (!data.success) {
        showToast(data.error || 'Failed to save credential', 'error');
        return false;
      }
    }

    closeCredentialForm();
    await loadCredentials();
    return true;
  } catch (error) {
    console.error('Failed to save credential:', error);
    showToast('Failed to save credential: ' + errorDetail(error), 'error');
    return false;
  }
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
    console.error('Failed to delete credential:', error);
    showToast('Failed to delete credential: ' + errorDetail(error), 'error');
  }
}
