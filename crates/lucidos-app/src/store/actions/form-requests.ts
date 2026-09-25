import { activeInlineForm, panelOverlay, showToast } from '../store';
import type { FormRequestEvent, FormRequestOutcome } from '../thread-events/thread-event-types';
import type { MessageOrigin } from '../../generated/thread-event-wire';
import { listPendingFormRequests } from '../../api/client/settings';
import { errorDetail } from '../../utils/errorDetail';
import { openCredentialRequest } from './credentials';
import { openPluginInstallRequest } from './plugin-install';
import { openPluginUninstallRequest } from './plugin-uninstall';
import { openEmailConfirmRequest } from './email-confirm';
import { getDeviceId } from './devices';
import {
  handleNavigationRequest,
  navigationIsForThisDevice,
  routeThreadNavigation,
} from './navigation-request';
import { formatThreadLabel } from './thread-label';

/** *Form requests*: something the agent asked the user to act on, persisted
 *  until a `FormRequestResolved` closes it (engine `engine/form_requests.rs`).
 *
 *  A live frame opens its form at once. This page can miss a frame: a replaced
 *  stream, a reconnect gap, a lagged broadcast, a reload. So
 *  `syncPendingFormRequests` runs on every stream open to find what it missed.
 *  The transcript row reopens one on demand. */

/** Requests this page load has already put in front of the user. A sync offers
 *  only what is not here, so a refocus never re-pops a form the user walked
 *  away from. A reload starts empty, and offers the open ones again. */
const offered = new Set<string>();

/** Test seam: forget what this page has offered. */
export function resetOfferedFormRequests(): void {
  offered.clear();
}

/** Open the form `event` asks for. The live frame, the pending sync and the
 *  transcript's Open button all come here.
 *
 *  `byUser` is the Open button. An authorization page the engine sent goes to
 *  one device, and never hijacks another thread's page. A user who pressed Open
 *  asked for it here and now. */
export function openFormRequest(
  threadId: string,
  event: FormRequestEvent,
  { byUser = false }: { byUser?: boolean } = {},
): void {
  offered.add(event.request_id);
  let payload: Record<string, unknown>;
  try {
    payload = JSON.parse(event.payload) as Record<string, unknown>;
  } catch (e) {
    showToast(`Failed to open the ${describeFormRequest(event)}: ${errorDetail(e)}`, 'error');
    return;
  }
  switch (event.type) {
    case 'CredentialRequested':
      openCredentialRequest({ ...payload, form_request_id: event.request_id });
      return;
    case 'EmailConfirmRequested':
      openEmailConfirmRequest({
        ...(payload as unknown as Parameters<typeof openEmailConfirmRequest>[0]),
        form_request_id: event.request_id,
      });
      return;
    case 'PluginInstallRequested':
      openPluginInstallRequest(payload as unknown as Parameters<typeof openPluginInstallRequest>[0]);
      return;
    case 'PluginUninstallRequested':
      openPluginUninstallRequest(
        payload as unknown as Parameters<typeof openPluginUninstallRequest>[0],
      );
      return;
    case 'OAuthAuthorizationRequested': {
      const nav = payload as unknown as Parameters<typeof handleNavigationRequest>[0];
      if (byUser) handleNavigationRequest(nav, { source: formatThreadLabel(threadId) });
      else routeThreadNavigation(nav, event.actor, threadId);
      return;
    }
  }
}

/** Whether this device should open `event` unasked. An authorization page goes
 *  to the device its actor names, as a navigation does. A form goes to every
 *  device, and the first answer closes it on the others. */
function isForThisDevice(event: FormRequestEvent): boolean {
  if (event.type !== 'OAuthAuthorizationRequested') return true;
  return navigationIsForThisDevice(event.actor);
}

/** Read the open requests and offer the newest one this page has not offered.
 *
 *  Called on every stream open, after the open, so a request emitted between
 *  the read and the open still arrives live. A form the user has open is
 *  never replaced: the rest stay reachable from their transcript rows. */
export async function syncPendingFormRequests(): Promise<void> {
  let pending;
  try {
    pending = await listPendingFormRequests();
  } catch (e) {
    // Runs on every stream open with no user action behind it. The next open
    // retries, and each open request stays reachable from its transcript row.
    // A toast here would only repeat itself on a flaky connection.
    console.warn('[FormRequests] reading the open form requests failed:', e);
    return;
  }
  if (activeInlineForm.value) return;
  const fresh = pending.filter((p) => !offered.has(p.request_id) && isForThisDevice(p.event));
  const newest = fresh[fresh.length - 1];
  if (newest) openFormRequest(newest.thread_id, newest.event);
}

/** Close the open form a resolved request was showing, when another device or
 *  path answered it.
 *
 *  **The acting device is exempt by its ACTOR.** Its own save, send or confirm
 *  already handles the form. A receipt it is about to stamp must not lose the
 *  race to this frame, and a receipt already on screen is left alone too.
 *
 *  A supersede is no such answer. A message typed on this device replaces the
 *  form, and nothing else here would close it. */
export function closeResolvedFormRequest(
  requestId: string,
  outcome: FormRequestOutcome,
  actor: MessageOrigin | null | undefined,
): void {
  const answeredHere = actor?.kind === 'device' && actor.device_id === getDeviceId();
  if (answeredHere && outcome !== 'superseded') return;
  const form = activeInlineForm.value;
  if (!form) return;
  const showing =
    (form.type === 'credential' && form.request?.form_request_id === requestId)
    || (form.type === 'email-confirm' && !form.sentAt && form.request.form_request_id === requestId)
    || (form.type === 'plugin-install' && !form.installed && form.request.install_id === requestId)
    || (form.type === 'plugin-uninstall' && !form.removed && form.request.uninstall_id === requestId);
  if (showing) panelOverlay.value = null;
}

/** The request in words, for a transcript row and an error toast. */
export function describeFormRequest(event: FormRequestEvent): string {
  switch (event.type) {
    case 'CredentialRequested': {
      const service = payloadField(event, 'service');
      return service ? `credential form for ${service}` : 'credential form';
    }
    case 'EmailConfirmRequested':
      return 'email confirmation';
    case 'PluginInstallRequested': {
      const name = payloadField(event, 'plugin_name') ?? payloadField(event, 'plugin_id');
      return name ? `install panel for ${name}` : 'plugin install panel';
    }
    case 'PluginUninstallRequested': {
      const name = payloadField(event, 'plugin_name') ?? payloadField(event, 'plugin_id');
      return name ? `uninstall panel for ${name}` : 'plugin uninstall panel';
    }
    case 'OAuthAuthorizationRequested':
      return 'authorization page';
  }
}

function payloadField(event: FormRequestEvent, key: string): string | undefined {
  try {
    const value = (JSON.parse(event.payload) as Record<string, unknown>)[key];
    return typeof value === 'string' && value ? value : undefined;
  } catch {
    // A payload that does not parse still has a generic name. Opening it is
    // where the parse failure is reported, with a toast.
    return undefined;
  }
}
