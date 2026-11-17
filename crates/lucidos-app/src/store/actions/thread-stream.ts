/** The eager transport admits restore generations before delayed store dispatch. */
import { API } from '../../api/client';
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

/** Keyed so each further stall updates one card, which admission removes. */
const STORAGE_STALL_TOAST_KEY = 'restore-admission-storage-stall';

/** Hellos failed by a device storage stall since the last admission. */
let storageStalls = 0;

let reconnectTimer: ReturnType<typeof setTimeout> | null = null;

/** A storage stall is not a restore failure. Its retry opens a fresh database
 *  connection, which usually answers, so only a repeat reaches the user. */
function reportAdmissionFailure(err: unknown): void {
  // The page hid mid-step and its next attempt retries; there is nothing to say.
  if (err instanceof DeviceDatabaseReleasedError) return;
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

/** Starts a new attempt at once. An open attempt may still wait out its
 *  storage bound, so it is dropped, with the frames it held back. */
function retryEventStreamNow(): void {
  disconnectThreadEvents({ preserveAdmitted: true });
  needsResyncOnOpen = true;
  connectThreadEvents();
}

/** Where the shell reaches the engine's event surface. The suffixes live in the
 *  SDK beside the transports, so the shell and an app cannot name them
 *  differently. */
function hostTargets(): EventStreamTargets {
  return eventStreamTargets(API);
}

function markEventStreamStatus(status: 'connecting' | 'connected' | 'disconnected'): void {
  if (typeof document === 'undefined') return;
  document.documentElement.dataset.lucidosEventStream = status;
}

/** Set when `onerror` fires, consumed by the next `onopen`, so the resync runs
 *  only on RECONNECT. On the initial connect `loadAllThreads()` is already
 *  driving state via startup.ts. */
let needsResyncOnOpen = false;

/** Generation counter to prevent stale EventSource handlers from interfering
 *  with newer connections. Incremented on each connectThreadEvents() call.
 *  onerror/onmessage handlers check their captured generation against the
 *  current value and bail if stale (old connection's handler firing after
 *  disconnectThreadEvents() + connectThreadEvents() replaced it). */
let sseGeneration = 0;

// Admitted work survives transport retries. Only an explicit teardown or a new
// restore generation invalidates it; a stale transport callback cannot add work.
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
    showToast(`Live updates could not be applied: ${errorDetail(error)}`, 'error', {
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
      // Stale handler: a newer connection replaced this one. The old transport
      // was closed, but its queued message handler can still fire.
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
          // A thread read ahead of admission may predate frames dropped since.
          if (consumeProvisionalReads()) needsResyncOnOpen = true;
          admittedOpen();
        } else if (admitted) dispatchFrame(data);
      }).catch((err: unknown) => {
        if (gen !== sseGeneration || frameEpoch !== restoreEpoch) return;
        // The disconnect below resets the phase, so name it now.
        postClientLog('restore-admission', 'failed', { phase: restoreAdmissionPhase(), error: errorDetail(err) });
        reportAdmissionFailure(err);
        // A connection sends one hello, so only a new one can retry admission.
        // Without it every gated request would wait forever.
        disconnectThreadEvents({ preserveAdmitted: true });
        // This connection dropped every frame it held back, so the retry
        // reconciles the gap. The disconnect above cleared the flag.
        needsResyncOnOpen = true;
        reconnectTimer = setTimeout(() => {
          reconnectTimer = null;
          connectThreadEvents();
        }, EVENT_STREAM_RETRY_MS);
      });
    },

    onOpen: () => {
      if (gen !== sseGeneration) return;
      // The first open reads what a frame emitted while no stream was up
      // would have refreshed. A reopen after an error reads it through the gap
      // reconcile, so it never runs both. On the first open startup.ts already
      // loads thread state, and a resync there would double-fetch every page
      // load.
      beginEpoch();
      if (admitted) admittedOpen();
    },

    onError: (drop: StreamDrop) => {
      // Stale handler: disconnectThreadEvents() already closed this transport
      // and connectThreadEvents() created a replacement. Without this guard the
      // old handler closes the NEW connection, via the module-scoped
      // `eventStream`, costing a 3s SSE gap on every iOS Safari PWA resume.
      if (gen !== sseGeneration) return;
      beginEpoch();

      // A plain error is routine: every engine restart causes one. Every other
      // drop is a failure only the client can see, so it reports it.
      if (drop.reason !== 'error') {
        const { reason, ...detail } = drop;
        postClientLog('event-stream', reason, detail);
      }

      // Mark for resync. Events emitted during the gap never reach this tab, so
      // the next successful connect must refetch persisted state.
      needsResyncOnOpen = true;
      // The engine may restart in the gap, and its backfill frames then count
      // from one again. A snapshot read meanwhile sets the new order.
      forgetTreeBackfillOrder();
      markEventStreamStatus('disconnected');
      fanOutEventStreamStatus('error');

      // A transport that retries for itself keeps its connection: the shared
      // worker owns the one upstream, and dropping our port would take that
      // stream down for every other document. Its next `open` lands here and
      // runs the resync armed above.
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

  // The shell answers a PresenceCheck, so it registers as a ponger. On a shared
  // connection the worker merges its ports' answers into the one pong the engine
  // waits for.
  // Remote engine bytes are data; only the connecting gateway supplies code.
  setEventStream(REMOTE_GATEWAY === null
    ? openEventStream(hostTargets(), handlers, { pongs: true })
    : openDirectEventStream(hostTargets(), handlers));
}

/** Make sure a stream is up after a resume, without cutting a healthy one.
 *
 *  A resume runs on every window focus, and a click that raises the window
 *  fires focus beside the action it caused. A frame the engine emits in a
 *  teardown gap is never replayed, so a navigate requested by that click would
 *  be lost. Only a missing or stale stream is replaced. A shared stream the
 *  worker finds silent reports through `onError`, then `onOpen`, and
 *  `reconcileAfterEventGap` repairs it there. */
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
  // Bump generation BEFORE closing, so any onerror handler queued by the
  // close() call sees a stale generation and bails out.
  sseGeneration++;
  // An explicit disconnect means the caller is taking ownership of state
  // recovery. Do not let the next onopen also resync, or every real reconnect
  // doubles the per-thread refreshThreadEvents fan-out.
  needsResyncOnOpen = false;
  // A resume sync disconnects here, often across an engine restart.
  forgetTreeBackfillOrder();
  if (reconnectTimer) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
  markEventStreamStatus('disconnected');
  // The closed transport's own error handler is now stale and stays silent, so
  // app frames hear the drop here. Their next `open` is the reconnect.
  fanOutEventStreamStatus('error');
  getEventStream()?.close();
  setEventStream(null);
}

