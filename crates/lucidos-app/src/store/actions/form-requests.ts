import { activeInlineForm, panelOverlay, showToast } from '../store';
import type { FormRequestEvent, FormRequestOutcome } from '../thread-events/thread-event-types';
import type { MessageOrigin } from '../../generated/thread-event-wire';
import { listPendingFormRequests, listResolvedFormRequests } from '../../api/client/settings';
import { errorDetail } from '../../utils/errorDetail';
import { openCredentialRequest } from './credentials';
import { openPluginInstallRequest } from './plugin-install';
import { openPluginUninstallRequest } from './plugin-uninstall';
import { openEmailConfirmRequest } from './email-confirm';
import { getDeviceId } from './devices';
import { isForThisDevice } from './device-scope';
import { submitInteractiveRequest, resolveInteractiveForm, synchronizeInteractiveForms, reconcileInteractiveForms, snapshotInteractiveForms } from '../interactiveDeliveryService';
import { handleNavigationRequest, presentThreadNavigation } from './navigation-request';
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
 *  away from. The shared journal retains presentation receipts across reloads. */
const offered = new Set<string>();

/** Test seam: forget what this page has offered. */
export function resetOfferedFormRequests(): void {
  offered.clear();
}

/** Open the form `event` asks for. The live frame, the pending sync and the
 *  transcript's Open button all come here.
 *
 *  `byUser` is the Open button. An authorization page the engine sent goes to
 *  one device, and never hijacks another thread's page. Any other unasked form
 *  opens through the device delivery journal. A user who pressed Open
 *  asked for it here and now. */
export function openFormRequest(
  threadId: string,
  event: FormRequestEvent,
  { byUser = false }: { byUser?: boolean } = {},
): void {
  if (byUser) { presentFormRequest(threadId, event, true); return; }
  if (!requestIsForThisDevice(event)) return;
  submitInteractiveRequest({ kind: 'form', payload: { threadId, event }, preference: { threadId } }, `form:${event.request_id}`);
}

/** Called synchronously only after a shared claim, or an explicit Open. */
export function presentFormRequest(threadId: string, event: FormRequestEvent, byUser = false): boolean {
  if (!byUser && activeInlineForm.value) return false;
  let payload: Record<string, unknown>;
  try {
    payload = JSON.parse(event.payload) as Record<string, unknown>;
  } catch (e) {
    showToast(`Failed to open the ${describeFormRequest(event)}: ${errorDetail(e)}`, 'error');
    return true;
  }
  offered.add(event.request_id);
  switch (event.type) {
    case 'CredentialRequested':
      openCredentialRequest({ ...payload, form_request_id: event.request_id });
      return true;
    case 'EmailConfirmRequested':
      openEmailConfirmRequest({
        ...(payload as unknown as Parameters<typeof openEmailConfirmRequest>[0]),
        form_request_id: event.request_id,
      });
      return true;
    case 'PluginInstallRequested':
      openPluginInstallRequest(payload as unknown as Parameters<typeof openPluginInstallRequest>[0]);
      return true;
    case 'PluginUninstallRequested':
      openPluginUninstallRequest(
        payload as unknown as Parameters<typeof openPluginUninstallRequest>[0],
      );
      return true;
    case 'OAuthAuthorizationRequested': {
      const nav = payload as unknown as Parameters<typeof handleNavigationRequest>[0];
      if (byUser) handleNavigationRequest(nav, { source: formatThreadLabel(threadId) });
      else presentThreadNavigation(nav, threadId);
      return true;
    }
  }
}

/** Whether this device should open `event` unasked. An authorization page goes
 *  to the device its actor names, as a navigation does. A form goes to every
 *  device, and the first answer closes it on the others. */
function requestIsForThisDevice(event: FormRequestEvent): boolean {
  if (event.type !== 'OAuthAuthorizationRequested') return true;
  return isForThisDevice(event.actor);
}

/** Queue open requests newest first, preserving older requests behind busy forms.
 *
 *  Called on every stream open, after the open, so a request emitted between
 *  the read and the open still arrives live. A form the user has open is
 *  never replaced: the rest stay reachable from their transcript rows. */
export async function syncPendingFormRequests(): Promise<void> {
  try { await synchronizeInteractiveForms(syncPendingFormRequestsPass); }
  catch (error) {
    showToast(`Could not reconcile interactive forms: ${errorDetail(error)}`, 'error', {
      key: 'interactive-form-reconciliation', action: { label: 'Retry', onClick: () => { void syncPendingFormRequests(); } },
    });
  }
}

async function syncPendingFormRequestsPass(): Promise<void> {
  const known = await snapshotInteractiveForms();
  let pending;
  try {
    pending = await listPendingFormRequests();
  } catch (e) {
    // Best-effort carve-out (frontend.md): runs on every stream open with no
    // user action behind it. The next open retries, and each open request
    // stays reachable from its transcript row.
    // A toast here would only repeat itself on a flaky connection.
    console.warn('[FormRequests] reading the open form requests failed:', e);
    return;
  }
  await reconcileInteractiveForms(known, pending.map((request) => request.request_id), await listResolvedFormRequests(known));
  const fresh = pending.filter((p) =>
    !offered.has(p.request_id)
    && requestIsForThisDevice(p.event));
  for (const request of fresh.reverse()) openFormRequest(request.thread_id, request.event);
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
  resolveInteractiveForm(requestId);
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
    case 'OAuthAuthorizationRequested': {
      const host = urlHost(payloadField(event, 'url'));
      return host ? `sign-in page on ${host}` : 'sign-in page';
    }
  }
}

/** The site a sign-in page lives on, which is what the reader recognises. */
function urlHost(url: string | undefined): string | undefined {
  if (!url) return undefined;
  try {
    return new URL(url).host || undefined;
  } catch {
    return undefined;
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
