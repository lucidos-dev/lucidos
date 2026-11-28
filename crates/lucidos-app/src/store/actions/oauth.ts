import { signal } from '@preact/signals';
import { submitInteractiveRequest } from '../interactiveDeliveryService';
import {
  knownOAuthProviders,
  oauthAccounts,
  showToast,
  showConfirm,
} from '../store';
import { toFailed, setLoadingIfFresh } from '../types';
import {
  listOAuthAccounts,
  listKnownOAuthProviders,
  deleteOAuthAccountApi,
  reauthorizeOAuth,
  completeOAuth,
} from '../../api/client';
import { openCredentialRequest } from './credentials';
import { openUrlOutsideApp } from './artifacts';
import { getDeviceId } from './devices';
import { isIOSPwa, isTauri } from '../../utils/platform';
import { focusCallingWindow } from '../../utils/tauri';
import { errorDetail } from '../../utils/errorDetail';

export interface OAuthAuthFlow {
  provider: string | null;
}

export const oauthAuthFlow = signal<OAuthAuthFlow | null>(null);

export function openOAuthAuthorizationUrl(authUrl: string, provider: string | null = null): void {
  oauthAuthFlow.value = { provider };
  openUrlOutsideApp(authUrl);
}

export function handleOAuthAccountConnected(payload: {
  provider?: string;
  email?: string | null;
  actor?: { kind?: string; device_id?: string } | null;
}, eventId?: string): void {
  if (oauthAccounts.value.status === 'loaded') void loadOAuthAccounts();

  const actor = payload.actor;
  if (actor?.kind !== 'device' || actor.device_id !== getDeviceId()) return;

  const flow = oauthAuthFlow.value;
  const startedHere =
    flow !== null && (flow.provider === null || flow.provider === payload.provider);
  submitInteractiveRequest({
    kind: 'oauth-connected', payload: { provider: payload.provider ?? null, email: payload.email ?? null },
    preference: { provider: payload.provider ?? null },
  }, eventId);
  oauthAuthFlow.value = null;

  if (startedHere && isTauri()) focusCallingWindow();
}

export function presentOAuthAccountConnected(payload: { provider?: string | null; email?: string | null }): void {
  const provider = payload.provider || 'Account';
  showToast(payload.email ? `${provider} connected (${payload.email})` : `${provider} connected`, 'success');
}

let pendingConnect: { provider: string; scopes: string; signIn: boolean } | null = null;

export function cancelPendingOAuthConnect(): void {
  pendingConnect = null;
}

export async function resumeOAuthConnectAfterCredentialSaved(service: string): Promise<boolean> {
  const pending = pendingConnect;
  if (!pending) return false;
  if (pending.provider.toLowerCase() !== service.trim().toLowerCase()) return false;
  pendingConnect = null;
  await grantOAuthScope(pending.provider, pending.scopes, pending.signIn);
  return true;
}

export async function loadKnownOAuthProviders(): Promise<void> {
  setLoadingIfFresh(knownOAuthProviders);
  try {
    knownOAuthProviders.value = { status: 'loaded', data: await listKnownOAuthProviders() };
  } catch (error) {
    knownOAuthProviders.value = toFailed(error);
  }
}

export async function loadOAuthAccounts(): Promise<void> {
  setLoadingIfFresh(oauthAccounts);
  try {
    const data = await listOAuthAccounts();
    oauthAccounts.value = { status: 'loaded', data: data.accounts || [] };
  } catch (error) {
    oauthAccounts.value = toFailed(error);
  }
}

export async function grantOAuthScope(
  provider: string,
  scopes: string,
  signIn = false,
): Promise<boolean> {
  if (isIOSPwa()) {
    showToast('OAuth connection is not available in the iOS app. Use the desktop browser instead.', 'error');
    return false;
  }
  try {
    const startResult = await reauthorizeOAuth(provider, scopes, signIn);
    if (startResult.credential_request) {
      pendingConnect = { provider, scopes, signIn };
      openCredentialRequest(startResult.credential_request);
      return false;
    }
    if (!startResult.success || !startResult.auth_url) {
      showToast(startResult.error || 'Failed to start OAuth flow', 'error');
      return false;
    }

    openOAuthAuthorizationUrl(startResult.auth_url, provider);

    const completeResult = await completeOAuth(provider);
    if (completeResult.success) {
      await loadOAuthAccounts();
      if (oauthAuthFlow.value !== null) {
        oauthAuthFlow.value = null;
        showToast(`${provider} connected`, 'success');
      }
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
  if (!(await showConfirm(`Disconnect ${provider} account?`, 'Disconnect', { variant: 'danger' }))) {
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
    showToast(`Failed to disconnect account: ${errorDetail(error)}`, 'error');
  }
}
