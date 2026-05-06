import { oauthAccounts, showToast, showConfirm } from '../store';
import { toFailed } from '../types';
import { listOAuthAccounts, deleteOAuthAccountApi, reauthorizeOAuth, completeOAuth } from '../../api/client';
import { openCredentialRequest } from './credentials';
import { isIOSPwa } from '../../utils/platform';
import { errorDetail } from '../../utils/errorDetail';

export async function loadOAuthAccounts(): Promise<void> {
  if (oauthAccounts.value.status !== 'loaded') {
    oauthAccounts.value = { status: 'loading' };
  }
  try {
    const data = await listOAuthAccounts();
    oauthAccounts.value = { status: 'loaded', data: data.accounts || [] };
  } catch (error) {
    console.error('Failed to load OAuth accounts:', error);
    oauthAccounts.value = toFailed(error);
  }
}

export async function grantOAuthScope(provider: string, scopes: string): Promise<boolean> {
  if (isIOSPwa()) {
    showToast('OAuth-tilkobling fungerer ikke i iOS-appen. Bruk desktop-nettleseren i stedet.', 'error');
    return false;
  }
  try {
    // Phase 1: Get the authorization URL from the backend
    const startResult = await reauthorizeOAuth(provider, scopes);
    if (startResult.credential_request) {
      openCredentialRequest(startResult.credential_request);
      return false;
    }
    if (!startResult.success || !startResult.auth_url) {
      showToast(startResult.error || 'Failed to start OAuth flow', 'error');
      return false;
    }

    // Phase 2: Open the auth URL in the user's browser
    window.open(startResult.auth_url, '_blank');

    // Phase 3: Wait for the callback to complete (blocks until user authorizes)
    const completeResult = await completeOAuth(provider);
    if (completeResult.success) {
      await loadOAuthAccounts();
      showToast('Account connected', 'success');
      return true;
    } else {
      showToast(completeResult.error || 'OAuth flow failed', 'error');
      return false;
    }
  } catch (error: unknown) {
    showToast(`Failed to connect account: ${errorDetail(error)}`, 'error');
    return false;
  }
}

export async function disconnectOAuthAccount(id: string, provider: string): Promise<void> {
  if (!(await showConfirm(`Disconnect ${provider} account?`, 'Disconnect'))) {
    return;
  }
  try {
    const data = await deleteOAuthAccountApi(id);
    if (data.success) {
      await loadOAuthAccounts();
      showToast('Account disconnected', 'success');
    } else {
      showToast(data.error || 'Failed to disconnect account', 'error');
    }
  } catch (error) {
    console.error('Failed to disconnect OAuth account:', error);
    showToast(`Failed to disconnect account: ${errorDetail(error)}`, 'error');
  }
}
