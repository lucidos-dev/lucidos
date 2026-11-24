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


const offered = new Set<string>();

export function resetOfferedFormRequests(): void {
  offered.clear();
}

export function openFormRequest(
  threadId: string,
  event: FormRequestEvent,
  { byUser = false }: { byUser?: boolean } = {},
): void {
  if (byUser) { presentFormRequest(threadId, event, true); return; }
  if (!requestIsForThisDevice(event)) return;
  submitInteractiveRequest({ kind: 'form', payload: { threadId, event }, preference: { threadId } }, `form:${event.request_id}`);
}

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

function requestIsForThisDevice(event: FormRequestEvent): boolean {
  if (event.type !== 'OAuthAuthorizationRequested') return true;
  return isForThisDevice(event.actor);
}

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
    console.warn('[FormRequests] reading the open form requests failed:', e);
    return;
  }
  await reconcileInteractiveForms(known, pending.map((request) => request.request_id), await listResolvedFormRequests(known));
  const fresh = pending.filter((p) =>
    !offered.has(p.request_id)
    && requestIsForThisDevice(p.event));
  for (const request of fresh.reverse()) openFormRequest(request.thread_id, request.event);
}

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
    return undefined;
  }
}
