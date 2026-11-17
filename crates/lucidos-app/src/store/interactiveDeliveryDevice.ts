/** Browser adapter for the journal protocol. BroadcastChannel is a wake-up,
 * while IndexedDB and a workspace/device Web Lock authorize presentation. */
import { isTauri } from '../utils/platform';
import { WORKSPACE_ID } from '../utils/basePath';
import { deviceDatabaseName } from './deviceDatabase';
import { createInteractiveDelivery, type InteractiveDeliveryDependencies } from './interactiveDelivery';
import { createInteractiveDeliveryBackend } from './interactiveDeliveryRecords';
import { holdPageOwnerLock, liveOwnerIds, pageOwnerId, PAGE_OWNER_LOCK_PREFIX } from './pageOwner';
import { assertNativeWindowShownReady, currentWindowView, observeWindowState, waitForNativeWindowShown } from './windowPeers';

export function interactiveDeliveryName(workspaceId: string | null, deviceId: string): string {
  return `${deviceDatabaseName('lucidos-interactive-delivery', workspaceId)}:device:${deviceId}`;
}

export function createDeviceInteractiveDelivery(options: {
  deviceId: string;
  ready?(): Promise<void>;
  present: InteractiveDeliveryDependencies['present'];
  uncertain: InteractiveDeliveryDependencies['uncertain'];
  failed(error: unknown): void;
}) {
  const name = interactiveDeliveryName(WORKSPACE_ID, options.deviceId);
  const lifetime = new AbortController();
  const watched = new Set<string>();
  const backend = createInteractiveDeliveryBackend(name);
  let channel: BroadcastChannel | null = null;
  try { if (typeof BroadcastChannel !== 'undefined') channel = new BroadcastChannel(name); }
  catch { /* The capability check below reports refused channels with Retry. */ }
  holdPageOwnerLock();

  function report(error: unknown): void {
    if (!lifetime.signal.aborted) {
      options.failed(error);
      // A failed receipt can leave this live page's claim uncertain too.
      void coordinator.reconsider().catch((recoveryError) => {
        if (!lifetime.signal.aborted) options.failed(recoveryError);
      });
    }
  }

  function watch(owner: string): void {
    if (owner === pageOwnerId || watched.has(owner)) return;
    watched.add(owner);
    void navigator.locks.request(PAGE_OWNER_LOCK_PREFIX + owner, { signal: lifetime.signal }, () => {
      watched.delete(owner);
      if (!lifetime.signal.aborted) void coordinator.reconsider().catch(report);
    }).catch(report);
  }

  const coordinator = createInteractiveDelivery({
    ownerId: pageOwnerId,
    backend: {
      async update(change) {
        const journal = await backend.update(change);
        for (const owner of Object.keys(journal.participants)) watch(owner);
        return journal;
      },
    },
    exclusive: async (body) => {
      if (typeof indexedDB === 'undefined' || typeof navigator === 'undefined' || !navigator.locks || !channel) {
        throw new Error('Interactive requests need IndexedDB, Web Locks and BroadcastChannel. Reopen Lucidos in a supported browser with device storage enabled.');
      }
      await options.ready?.();
      if (isTauri()) {
        await waitForNativeWindowShown();
        assertNativeWindowShownReady();
      }
      return navigator.locks.request(name, { signal: lifetime.signal }, body);
    },
    liveOwners: liveOwnerIds,
    readSelf: () => {
      const view = currentWindowView();
      if (isTauri()) assertNativeWindowShownReady();
      return view;
    },
    present: options.present,
    uncertain: options.uncertain,
    changed: () => { if (!lifetime.signal.aborted) channel?.postMessage('changed'); },
    now: Date.now,
  });
  if (channel) channel.onmessage = () => { void coordinator.reconsider().catch(report); };
  const stopObserving = observeWindowState(() => { void coordinator.publish().catch(report); });

  return {
    ...coordinator,
    async synchronizeForms(body: () => Promise<void>): Promise<void> {
      if (typeof navigator === 'undefined' || !navigator.locks) {
        throw new Error('Interactive forms need Web Locks. Reopen Lucidos in a supported browser.');
      }
      await navigator.locks.request(`${name}:form-sync`, { signal: lifetime.signal }, body);
    },
    async stop() {
      stopObserving();
      try { await coordinator.stop(); }
      finally {
        lifetime.abort();
        channel?.close();
      }
    },
  };
}
