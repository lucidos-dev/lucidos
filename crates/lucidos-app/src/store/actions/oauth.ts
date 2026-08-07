import { signal } from '@preact/signals';
import {
  knownOAuthProviders,
  oauthAccounts,
  panelOverlay,
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
import { openUrl } from './artifacts';
import { getDeviceId } from './devices';
import { isIOSPwa, isTauri } from '../../utils/platform';
import { focusCallingWindow } from '../../utils/tauri';
import { errorDetail } from '../../utils/errorDetail';

/** The OAuth authorization THIS PAGE last handed to `openUrl`, or null. */
export interface OAuthAuthFlow {
  /** The authorization URL, so the in-app browser panel showing it can be
   *  matched and closed once the flow lands. */
  url: string;
  /** Which provider this flow is authorizing, when the caller knew. The
   *  Settings buttons do; the engine's `NavigationRequested` carries no
   *  provider, so that path records null and any completion counts as this
   *  page's. */
  provider: string | null;
}

/** The authorization this page last opened, or null.
 *
 *  Exists so the in-app browser panel can be closed once the flow lands. When
 *  the user has the in-app browser on, the authorization page opens in the
 *  url-preview panel and the provider redirects it to the loopback callback, so
 *  when the connection completes the user is left looking at a dead callback
 *  page inside the app with no reason to still be there. It is also what tells
 *  `handleOAuthAccountConnected` that THIS page started the flow, which is
 *  narrower than the device the engine stamps on the event.
 *
 *  The panel is matched by URL rather than by a boolean "flow in flight" flag
 *  deliberately: a flag goes stale when a flow times out or is abandoned, and
 *  would then close an unrelated page the user opened later. A stale URL can
 *  only ever match the authorization page itself, so it needs no fuse. */
export const oauthAuthFlow = signal<OAuthAuthFlow | null>(null);

/** Record the authorization about to be opened, then open it. Both entry points
 *  route through here (the Settings Connect / Reconnect buttons, and the
 *  engine's `NavigationRequested` when the agent runs `connect_oauth_account`)
 *  so neither can forget the half that lets the panel close itself later. */
export function openOAuthAuthorizationUrl(authUrl: string, provider: string | null = null): void {
  oauthAuthFlow.value = { url: authUrl, provider };
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
 *  consent screen and are left staring at a callback tab.
 *
 *  The FRONTING is narrower than the device: it happens only in the page that
 *  opened THIS authorization. A device id is per-app, not per-window (it comes
 *  from the Tauri `get_or_create_device_id` command), so every window on this
 *  workspace runs this handler and a window with nothing to do with the
 *  authorization would otherwise jump forward too. `oauthAuthFlow` is the
 *  per-page record of what this page opened, and the provider is compared as
 *  well as its presence: the engine allows ONE live callback flow at a time
 *  (`core::oauth` `ACTIVE_CALLBACK_FLOW`), so a second Connect supersedes the
 *  first and leaves the window holding the dead one with a marker it should not
 *  be fronted on. A flow recorded without a provider keeps the looser "any
 *  completion is mine", which is all the agent path can say.
 *
 *  The CLEAR stays unconditional, on purpose. The window whose flow was
 *  superseded is exactly the one left with a dead authorization page in its
 *  panel, and dropping the marker is what closes it.
 *
 *  The toast deliberately stays device-scoped rather than following the
 *  fronting. It costs nothing in a second window, and a page that reloaded
 *  mid-authorization has lost the marker: narrowing it too would trade a
 *  visible window bug for a silent missing notice. */
export function handleOAuthAccountConnected(payload: {
  provider?: string;
  email?: string | null;
  actor?: { kind?: string; device_id?: string } | null;
}): void {
  if (oauthAccounts.value.status === 'loaded') void loadOAuthAccounts();

  const actor = payload.actor;
  if (actor?.kind !== 'device' || actor.device_id !== getDeviceId()) return;

  const flow = oauthAuthFlow.value;
  const startedHere =
    flow !== null && (flow.provider === null || flow.provider === payload.provider);
  const overlay = panelOverlay.value;
  if (flow && overlay?.type === 'url-preview' && overlay.url === flow.url) {
    panelOverlay.value = null;
  }
  oauthAuthFlow.value = null;

  const provider = payload.provider || 'Account';
  const email = payload.email;
  showToast(email ? `${provider} connected (${email})` : `${provider} connected`, 'success');

  if (startedHere && isTauri()) focusCallingWindow();
}

/** The authorization a credential save should continue into, or null.
 *
 *  `grantOAuthScope` returns `false` when the engine answers "no OAuth client
 *  yet" (or "the one you have cannot drive a flow"), because all it can do at
 *  that moment is open the registration form. Nothing used to survive that
 *  return, so saving the credential closed the form, reloaded the list, and
 *  stopped: no one told the user to press Connect again, and no one did it for
 *  them. This is the missing half.
 *
 *  Module-scoped rather than a signal because nothing renders it, and it is
 *  deliberately single-slot: the engine allows one live authorization at a time
 *  (`ACTIVE_CALLBACK_FLOW`), so a second Connect supersedes the first here too.
 *
 *  Cleared before the resume runs, so a failed continuation cannot re-fire on
 *  the next unrelated credential save. */
let pendingConnect: { provider: string; scopes: string } | null = null;

/** Abandon any queued continuation. Called when the registration form is
 *  dismissed: a cancelled form must not open a browser. */
export function cancelPendingOAuthConnect(): void {
  pendingConnect = null;
}

/** Continue the authorization the saved credential was blocking, if any.
 *
 *  `service` is the credential that was just saved, and it MUST match the queued
 *  provider or nothing runs. A no-op is the ordinary case: an agent-driven
 *  `request_credential`, or a credential added from the Add Credential button,
 *  has no flow waiting behind it.
 *
 *  Matching on the name rather than just "something is queued" is what stops a
 *  continuation the user walked away from being consumed by an unrelated
 *  request later: navigating out of the form does not dismiss it, so the queued
 *  entry can outlive the screen that created it, and opening a browser for a
 *  provider the user abandoned is worse than doing nothing. Compared
 *  case-insensitively, since the engine lowercases an OAuth client's name.
 *
 *  Returns whether it ran, for tests. */
export async function resumeOAuthConnectAfterCredentialSaved(service: string): Promise<boolean> {
  const pending = pendingConnect;
  if (!pending) return false;
  if (pending.provider.toLowerCase() !== service.trim().toLowerCase()) return false;
  pendingConnect = null;
  await grantOAuthScope(pending.provider, pending.scopes);
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

export async function grantOAuthScope(provider: string, scopes: string): Promise<boolean> {
  if (isIOSPwa()) {
    showToast('OAuth connection is not available in the iOS app. Use the desktop browser instead.', 'error');
    return false;
  }
  try {
    // Phase 1: Get the authorization URL from the backend
    const startResult = await reauthorizeOAuth(provider, scopes);
    if (startResult.credential_request) {
      // The registration is missing or incomplete, so the form opens. Remember
      // what this press was FOR, so saving it continues straight into the
      // browser rather than ending here with the user on the Accounts page
      // wondering what to do next.
      pendingConnect = { provider, scopes };
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
    // when it's off. A raw `window.open` ignored both. The provider is the same
    // string the engine stores and echoes on `OAuthAccountConnected`, so the
    // completion can be matched back to this flow.
    openOAuthAuthorizationUrl(startResult.auth_url, provider);

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
      // it clears `oauthAuthFlow` whenever it runs on this page, so a marker
      // still sitting there is exactly "nobody reported this".
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
