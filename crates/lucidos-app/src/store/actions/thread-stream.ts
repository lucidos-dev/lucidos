import { API, isTransientFetchError } from '../../api/client';
import { beginRestoreConnection, acceptRestoreHello, hasRestoreRuntime } from '../restoreRuntime';
import { noteRestoreAdmitted, stopWatchingRestoreAdmission, watchRestoreAdmission } from '../restoreAdmissionWatchdog';
import { DeviceDatabaseReleasedError, DeviceDatabaseStalledError } from '../deviceDatabase';
import { consumeProvisionalReads, restoreAdmissionPhase } from '../../utils/restoreAdmission';
import { EVENT_STREAM_RETRY_MS, eventStreamTargets, openEventStream, openDirectEventStream, type EventStreamTargets, type StreamDrop } from '@lucidos/event-stream';
import { REMOTE_GATEWAY } from '../../utils/basePath';
import { postClientLog } from '../../utils/clientLog';
import { getEventStream, noteEventFrame, setEventStream } from './event-stream';
import { fanOutEventFrame, fanOutEventStreamStatus } from './app-frame-channel';
import { showToast, removeToast } from '../store';
import { errorDetail } from '../../utils/errorDetail';
import { forgetTreeBackfillOrder } from './treeBackfill';
import { lazyModule } from '../../utils/lazyComponent';
import { prefetchWhenIdle } from '../../utils/idlePrefetch';

const eventSink = lazyModule(() => import('./thread-sync'));

const STORAGE_STALL_TOAST_KEY = 'restore-admission-storage-stall';

let storageStalls = 0;

let reconnectTimer: ReturnType<typeof setTimeout> | null = null;

function reportAdmissionFailure(err: unknown): void {
  if (err instanceof DeviceDatabaseReleasedError || isTransientFetchError(err)) return;
  if (!(err instanceof DeviceDatabaseStalledError)) {
    showToast(`Workspace restore recovery could not finish: ${errorDetail(err)}`, 'error', { title: 'Workspace recovery' });
    return;
  }
  if (++storageStalls < 2) return;
  showToast("This device's storage did not respond. You can read, but sending waits until it answers. Lucidos keeps trying.", 'warning', {
    key: STORAGE_STALL_TOAST_KEY,
    title: 'Device storage',
    action: { label: 'Retry', onClick: retryEventStreamNow },
  });
}

function retryEventStreamNow(): void {
  disconnectThreadEvents({ preserveAdmitted: true });
  needsResyncOnOpen = true;
  connectThreadEvents();
}

function hostTargets(): EventStreamTargets {
  return eventStreamTargets(API);
}

function markEventStreamStatus(status: 'connecting' | 'connected' | 'disconnected'): void {
  if (typeof document === 'undefined') return;
  document.documentElement.dataset.lucidosEventStream = status;
}

let needsResyncOnOpen = false;

let sseGeneration = 0;

let dispatchEpoch = 0;
let lastAdmittedGeneration: string | null | undefined;
const pending: Array<(sink: typeof import('./thread-sync')) => void> = [];
let draining = false;
let sinkReady = false;
const drain = () => {
  if (draining) return;
  const sink = eventSink.get();
  draining = true;
  try {
    while (pending.length) pending.shift()!(sink);
  } catch (error) {
    const storageQuotaExceeded = error instanceof DOMException && error.name === 'QuotaExceededError';
    const message = storageQuotaExceeded
      ? 'Lucidos reached a storage limit on this device. Some threads may be out of date. Try reconnecting to refresh them.'
      : `Live updates could not be applied: ${errorDetail(error)}`;
    showToast(message, 'error', {
      title: storageQuotaExceeded ? 'Live updates interrupted' : undefined,
      key: 'event-dispatch-error', action: { label: 'Reconnect', onClick: retryEventStreamNow },
    });
  } finally { draining = false; }
};

const preload = async () => {
  const epoch = dispatchEpoch;
  if (!await eventSink.preload()) {
    if (epoch !== dispatchEpoch) return;
    showToast('Live update code could not load. Your updates are waiting.', 'error', {
      key: 'event-dispatch-error', action: { label: 'Retry', onClick: () => { void preload(); } },
    });
    return;
  }
  if (epoch !== dispatchEpoch) return;
  sinkReady = true;
  removeToast('event-dispatch-error');
  drain();
};
const enqueue = (task: (sink: typeof import('./thread-sync')) => void) => {
  pending.push(task);
  if (sinkReady) drain();
};

export function connectThreadEvents(): void {
  if (getEventStream()) return;

  const gen = ++sseGeneration;
  sinkReady ||= eventSink.isLoaded();
  const epoch = dispatchEpoch;
  prefetchWhenIdle({ preload: () => epoch === dispatchEpoch ? preload() : Promise.resolve() });
  let restoreEpoch = 0;
  let admitted = true;
  const beginEpoch = () => {
    restoreEpoch = beginRestoreConnection();
    admitted = !hasRestoreRuntime();
    if (!admitted) watchRestoreAdmission(() => { if (gen === sseGeneration) getEventStream()?.restart(); });
  };
  beginEpoch();
  let frames: Promise<void> = Promise.resolve();
  markEventStreamStatus('connecting');

  const admittedOpen = () => {
    if (gen !== sseGeneration) return;
    markEventStreamStatus('connected');
    fanOutEventStreamStatus('open');
    const reopenedAfterError = needsResyncOnOpen;
    needsResyncOnOpen = false;
    enqueue((sink) => sink.onStreamOpen(reopenedAfterError));
  };
  const dispatchFrame = (data: string) => {
    fanOutEventFrame(data);
    enqueue((sink) => sink.handleHostFrame(data));
  };
  const handlers = {
    onFrame: (data: string) => {
      if (gen !== sseGeneration) return;
      noteEventFrame();
      if (!hasRestoreRuntime()) { dispatchFrame(data); return; }
      const frameEpoch = restoreEpoch;
      frames = frames.then(async () => {
        if (gen !== sseGeneration || frameEpoch !== restoreEpoch) return;
        const frame = JSON.parse(data) as { type?: string; data?: { restore_generation?: unknown } };
        if (frame.type === 'StreamHello') {
          const generation = frame.data?.restore_generation;
          if (generation !== null && (typeof generation !== 'string'
            || !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(generation))) {
            throw new Error('The event stream did not identify its restore generation');
          }
          const accepted = await acceptRestoreHello(frameEpoch, generation);
          if (!accepted || gen !== sseGeneration || frameEpoch !== restoreEpoch) return;
          if (lastAdmittedGeneration !== undefined && lastAdmittedGeneration !== generation) pending.length = 0;
          lastAdmittedGeneration = generation as string | null;
          admitted = true;
          noteRestoreAdmitted();
          storageStalls = 0;
          removeToast(STORAGE_STALL_TOAST_KEY);
          enqueue((sink) => sink.flushAdmittedComposeDrafts());
          if (consumeProvisionalReads()) needsResyncOnOpen = true;
          admittedOpen();
        } else if (admitted) dispatchFrame(data);
      }).catch((err: unknown) => {
        if (gen !== sseGeneration || frameEpoch !== restoreEpoch) return;
        postClientLog('restore-admission', 'failed', { phase: restoreAdmissionPhase(), error: errorDetail(err) });
        reportAdmissionFailure(err);
        disconnectThreadEvents({ preserveAdmitted: true });
        needsResyncOnOpen = true;
        reconnectTimer = setTimeout(() => {
          reconnectTimer = null;
          connectThreadEvents();
        }, EVENT_STREAM_RETRY_MS);
      });
    },

    onOpen: () => {
      if (gen !== sseGeneration) return;
      beginEpoch();
      if (admitted) admittedOpen();
    },

    onError: (drop: StreamDrop) => {
      if (gen !== sseGeneration) return;
      beginEpoch();

      if (drop.reason !== 'error') {
        const { reason, ...detail } = drop;
        postClientLog('event-stream', reason, detail);
      }

      needsResyncOnOpen = true;
      forgetTreeBackfillOrder();
      markEventStreamStatus('disconnected');
      fanOutEventStreamStatus('error');

      if (getEventStream()?.ownsReconnect) return;

      getEventStream()?.close();
      setEventStream(null);
      if (reconnectTimer) clearTimeout(reconnectTimer);
      reconnectTimer = setTimeout(() => {
        reconnectTimer = null;
        connectThreadEvents();
      }, EVENT_STREAM_RETRY_MS);
    },
  };

  setEventStream(REMOTE_GATEWAY === null
    ? openEventStream(hostTargets(), handlers, { pongs: true })
    : openDirectEventStream(hostTargets(), handlers));
}

export function reviveThreadEvents(): void {
  const stream = getEventStream();
  if (stream && stream.revalidate() !== 'stale') return;
  disconnectThreadEvents({ preserveAdmitted: true });
  connectThreadEvents();
}

export function disconnectThreadEvents({ preserveAdmitted = false }: { preserveAdmitted?: boolean } = {}): void {
  if (!preserveAdmitted) {
    dispatchEpoch++;
    sinkReady = false;
    pending.length = 0;
    lastAdmittedGeneration = undefined;
  }
  beginRestoreConnection();
  stopWatchingRestoreAdmission();
  sseGeneration++;
  needsResyncOnOpen = false;
  forgetTreeBackfillOrder();
  if (reconnectTimer) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
  markEventStreamStatus('disconnected');
  fanOutEventStreamStatus('error');
  getEventStream()?.close();
  setEventStream(null);
}

