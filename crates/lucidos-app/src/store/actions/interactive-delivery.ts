/** Startup dispatcher. Direct presenters do not enqueue another claim. */
import { effect } from '@preact/signals';
import { activeInlineForm, confirmState, ensureAskDialogsReady, showConfirm, showToast } from '../store';
import { installInteractiveDeliveryService } from '../interactiveDeliveryService';
import type { InteractiveRequest } from '../interactiveDeliveryRecords';
import type { FormRequestEvent } from '../thread-events/thread-event-types';
import { presentFormRequest } from './form-requests';
import { presentThreadNavigation, type handleNavigationRequest } from './navigation-request';
import { presentOAuthAccountConnected } from './oauth';
import { getDeviceId } from './devices';
import { setDevicePushEnabled } from './push';
import { errorDetail } from '../../utils/errorDetail';
import { lazyModule } from '../../utils/lazyComponent';

const deviceAdapter = lazyModule(() => import('../interactiveDeliveryDevice'));
type DeviceCoordinator = ReturnType<typeof import('../interactiveDeliveryDevice')['createDeviceInteractiveDelivery']>;

export function presentInteractiveRequest(request: InteractiveRequest): boolean {
  const payload = request.payload;
  switch (request.kind) {
    case 'form':
      return presentFormRequest(payload.threadId as string, payload.event as FormRequestEvent);
    case 'navigation':
      presentThreadNavigation(payload.nav as Parameters<typeof handleNavigationRequest>[0], payload.sourceThreadId as string);
      return true;
    case 'oauth-connected':
      presentOAuthAccountConnected(payload);
      return true;
    case 'push-enable':
      if (confirmState.peek().visible) return false;
      // Opening is synchronous after startup prepares the dialog chunk. The
      // answer and permission request run after the claim has been released.
      void showConfirm('Enable push notifications?', 'Enable', { variant: 'default' })
        .then(async (ok) => { if (ok) await setDevicePushEnabled(getDeviceId(), true); })
        .catch((error) => showToast(`Failed to enable push notifications: ${errorDetail(error)}`, 'error'));
      return true;
  }
}

export function installInteractiveDelivery(register = installInteractiveDeliveryService): () => void {
  let stopped = false;
  const failed = (error: unknown, retry: () => void) => {
    if (stopped) return;
    showToast(`Interactive request delivery failed: ${errorDetail(error)}`, 'error',
      { key: 'interactive-delivery-error', action: { label: 'Retry', onClick: retry } });
  };
  const deviceId = getDeviceId();
  let ready: Promise<DeviceCoordinator> | null = null;
  async function initialize(): Promise<DeviceCoordinator> {
    if (!await deviceAdapter.preload()) throw new Error('Delivery code could not load. Retry or refresh Lucidos.');
    return deviceAdapter.get().createDeviceInteractiveDelivery({
      deviceId,
      ready: ensureAskDialogsReady,
      present: presentInteractiveRequest,
      uncertain: (request) => showToast('An interactive request may already have opened. Retry only if you still need it.', 'warning', {
        key: `interactive-uncertain-${request.id}`,
        action: { label: 'Retry', onClick: () => { void coordinator.retry(request.id).catch((error) => failed(error, reconsider)); } },
      }),
      failed: (error) => failed(error, reconsider),
    });
  }
  function assertRunning(): void {
    if (stopped) throw new Error('Interactive delivery stopped. Reopen Lucidos.');
  }
  async function run<T>(action: (current: DeviceCoordinator) => Promise<T>): Promise<T> {
    assertRunning();
    const current = await (ready ??= initialize().catch((error) => { ready = null; throw error; }));
    assertRunning();
    return action(current);
  }
  const coordinator = {
    formIds: () => run((current) => current.formIds()),
    synchronizeForms: (body: () => Promise<void>) => run((current) => current.synchronizeForms(body)),
    reconcileForms: (known: string[], open: string[], resolved: string[]) => run((current) => current.reconcileForms(known, open, resolved)),
    submit: (request: InteractiveRequest) => run((current) => current.submit(request)),
    resolve: (id: string) => run((current) => current.resolve(id)),
    publish: () => run((current) => current.publish()),
    reconsider: () => run((current) => current.reconsider()),
    retry: (id: string) => run((current) => current.retry(id)),
  };
  function reconsider(): void { void coordinator.reconsider().catch((error) => failed(error, reconsider)); }
  const uninstall = register(coordinator, failed);
  // Busy forms and confirms decline a claim. Closing them is meaningful state
  // and must retry pending work without waiting for a new SSE connection.
  const stopBusy = effect(() => {
    void activeInlineForm.value;
    void confirmState.value;
    void coordinator.publish().catch((error) => {
      failed(error, reconsider);
      reconsider();
    });
  });
  return () => {
    stopped = true;
    stopBusy();
    uninstall();
    void ready?.then((current) => current.stop().catch((error) => {
      showToast(`Interactive delivery cleanup failed: ${errorDetail(error)}`, 'error');
    })).catch((error) => failed(error, reconsider));
  };
}
