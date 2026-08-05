import { signal } from '@preact/signals';
import { oauthAccounts, panelOverlay, showToast, showConfirm } from '../store';
import { toFailed, setLoadingIfFresh } from '../types';
import { listOAuthAccounts, deleteOAuthAccountApi, reauthorizeOAuth, completeOAuth } from '../../api/client';
import { openCredentialRequest } from './credentials';
import { openUrl } from './artifacts';
import { getDeviceId } from './devices';
import { isIOSPwa, isTauri } from '../../utils/platform';
import { focusMainWindow } from '../../utils/tauri';
import { errorDetail } from '../../utils/errorDetail';

/** The authorization URL this device last handed to `openUrl` for an OAuth
 *  flow, or null.
 *
 *  Exists so the in-app browser panel can be closed once the flow lands. When
 *  the user has the in-app browser on, the authorization page opens in the
 *  url-preview panel and the provider redirects it to the loopback callback, so
 *  when the connection completes the user is left looking at a dead callback
 *  page inside the app with no reason to still be there.
 *
 *  Matched by URL rather than a boolean "flow in flight" flag deliberately: a
 *  flag goes stale when a flow times out or is abandoned, and would then close
 *  an unrelated page the user opened later. A stale URL can only ever match the
 *  authorization page itself, so it needs no fuse. */
export const oauthAuthPanelUrl = signal<string | null>(null);

/** Record the authorization URL about to be opened, then open it. Both entry
 *  points route through here (the Settings Connect / Reconnect buttons, and the
 *  engine's `NavigationRequested` when the agent runs `connect_oauth_account`)
 *  so neither can forget the half that lets the panel close itself later. */
export function openOAuthAuthorizationUrl(authUrl: string): void {
  oauthAuthPanelUrl.value = authUrl;
  openUrl(authUrl);
}

/** SSE handler for `OAuthAccountConnected`, the moment an authorization the user
 *  completed in a browser lands back in the engine.
 *
 *  Everything past the accounts reload is scoped to the device that STARTED the
 *  flow (the engine stamps it as the event's actor; see `prepare_oauth_flow`).
 *  An account connected from a phone must not front a desktop window, and the
 *  desktop's dead callback panel is not the phone's problem either. An event
 *  with no actor (an engine-internal reconnect) scopes to nobody.
 *
 *  For the initiating device: close the callback page if it is sitting in the
 *  in-app browser panel, say which account connected, and bring the window back
 *  to the front. That last part is why the whole actor thread exists: the user's
 *  attention is in a browser at that moment, and without it they approve the
 *  consent screen and are left staring at a callback tab. */
export function handleOAuthAccountConnected(payload: {
  provider?: string;
  email?: string | null;
  actor?: { kind?: string; device_id?: string } | null;
}): void {
  if (oauthAccounts.value.status === 'loaded') void loadOAuthAccounts();

  const actor = payload.actor;
  if (actor?.kind !== 'device' || actor.device_id !== getDeviceId()) return;

  const authUrl = oauthAuthPanelUrl.value;
  const overlay = panelOverlay.value;
  if (authUrl && overlay?.type === 'url-preview' && overlay.url === authUrl) {
    panelOverlay.value = null;
  }
  oauthAuthPanelUrl.value = null;

  const provider = payload.provider || 'Account';
  const email = payload.email;
  showToast(email ? `${provider} connected (${email})` : `${provider} connected`, 'success');

  if (isTauri()) focusMainWindow();
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

export async function grantOAuthScope(provider: string, scopes: string): Promise<boolean> {
  if (isIOSPwa()) {
    showToast('OAuth connection is not available in the iOS app. Use the desktop browser instead.', 'error');
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

    // Phase 2: Open the auth URL wherever the user has configured links to open.
    // Not a bare `window.open`: the desktop app with the in-app browser
    // preference on wants the panel, and the OS opener is the correct target
    // when it's off. A raw `window.open` ignored both.
    openOAuthAuthorizationUrl(startResult.auth_url);

    // Phase 3: Wait for the callback to complete (blocks until user authorizes)
    const completeResult = await completeOAuth(provider);
    if (completeResult.success) {
      await loadOAuthAccounts();
      // `handleOAuthAccountConnected` normally owns the success toast (it also
      // closes the panel and fronts the window, and it knows the email, which
      // this response does not). But it is device-scoped, so it stays silent if
      // the event's actor isn't this device or the SSE never arrived, and a
      // user who CLICKED Connect must not be left with a button that merely
      // stopped spinning. Fall back only when the handler didn't run for us:
      // it clears `oauthAuthPanelUrl` on the matching branch, so a value still
      // sitting there is exactly "nobody reported this".
      if (oauthAuthPanelUrl.value !== null) {
        oauthAuthPanelUrl.value = null;
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
    showToast(`Failed to disconnect account: ${errorDetail(error)}`, 'error');
  }
}
