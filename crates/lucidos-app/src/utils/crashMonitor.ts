/** Temporary diagnostics: docs/temporary-measures.md § Page health log. */
import { requestsInFlight } from '../api/client/requestsInFlight';
import { restoreAdmissionPhase, restoreAdmissionReady } from './restoreAdmission';
import { effect } from '@preact/signals';
import { focusedThreadId } from '../store/store';
import { crashStore } from './crashBreadcrumbs';
import { onCrashWorker } from './crashWorker';
import { onCrashPageHide } from './crashPageHide';
import { preserveCrashRecord } from './crashStorage';
import { onPageHide, onPageWake } from './pageVisit';
import { pageResources } from './pageResources';
import { postClientLog } from './clientLog';

const WAKE_WINDOW_MS = 15_000;
const WAKE_INTERVAL_MS = 500;
const STREAM_STATES = ['connecting', 'connected', 'disconnected'];
const ADMISSION_PHASES = ['awaiting-hello', 'generation', 'local-records', 'historical-sources', 'journal', 'recoveries', 'draft-recovery', 'admitted'];

const DENSE_WINDOW_MS = 20_000;
const DENSE_INTERVAL_MS = 2000;
const SAMPLE_INTERVAL_MS = 10_000;
const MOUNT_DEBOUNCE_MS = 250;
// A decoded area threshold for diagnostics, not a process-memory estimate.
const LARGE_IMAGE_PIXELS = 512 * 512;

/** One bounded observer for mount events; never retains mounted elements. */
export function startCrashMonitor(): { stop: () => void; stall: (lag: number) => void } {
  preserveCrashRecord();
  const store = crashStore();
  const mark = (kind: Parameters<typeof store.add>[0], data = {}) => { store.add(kind, data); store.flush(); };
  const sample = () => {
    const started = performance.now();
    observeAdmission();
    const resources = pageResources(focusedThreadId.peek());
    const requests = requestsInFlight();
    return { ...resources, inflight: requests.count, oldestMs: Math.max(0, requests.oldestMs ?? 0), costMs: Math.max(0, performance.now() - started) };
  };
  const capture = () => { store.add('sample', sample()); store.flush(); };
  let denseUntil = 0;
  let wakeUntil = 0;
  let transcriptSeen = false;
  let timer: ReturnType<typeof setTimeout>;
  let mountTimer: ReturnType<typeof setTimeout> | undefined;
  const tick = () => {
    if (document.visibilityState === 'visible') capture();
    schedule();
  };
  const schedule = () => {
    const now = performance.now();
    const wakeRemaining = wakeUntil - now;
    const remaining = denseUntil - now;
    timer = setTimeout(tick, wakeRemaining > 0 ? Math.min(WAKE_INTERVAL_MS, wakeRemaining)
      : remaining > 0 ? Math.min(DENSE_INTERVAL_MS, remaining) : SAMPLE_INTERVAL_MS);
  };
  const captureMount = () => {
    if (performance.now() >= Math.max(denseUntil, wakeUntil) || document.visibilityState !== 'visible' || mountTimer !== undefined) return;
    // Coalesce a batch without postponing forever during continuous streaming.
    mountTimer = setTimeout(() => {
      mountTimer = undefined;
      if (document.visibilityState === 'visible') capture();
    }, MOUNT_DEBOUNCE_MS);
  };
  const largeImage = (node: Element) => node instanceof HTMLImageElement && node.naturalWidth * node.naturalHeight >= LARGE_IMAGE_PIXELS;
  const startDenseWindow = () => {
    clearTimeout(timer);
    clearTimeout(mountTimer);
    mountTimer = undefined;
    denseUntil = performance.now() + DENSE_WINDOW_MS;
    transcriptSeen = false;
    schedule();
  };
  // Focus changes before the new transcript commits; record the transition
  // without attributing the old document's resource counts to the new thread.
  const stopFocus = effect(() => {
    void focusedThreadId.value;
    startDenseWindow();
    store.add('thread-open', {});
    store.flush();
  });
  let lastPhase = -1;
  const observeAdmission = () => {
    const phase = ADMISSION_PHASES.indexOf(restoreAdmissionPhase());
    if (phase !== lastPhase) { lastPhase = phase; mark('admission', { phase }); }
  };
  const stopAdmission = effect(() => {
    restoreAdmissionReady();
    observeAdmission();
  });
  let lastStream = -1;
  const observeStream = () => {
    const state = STREAM_STATES.indexOf(document.documentElement.dataset.lucidosEventStream ?? '') + 1;
    if (state !== lastStream) { lastStream = state; mark('stream', { state }); }
  };
  observeStream();
  const observer = new MutationObserver(records => {
    if (records.some(record => record.type === 'attributes')) observeStream();
    // Mount counters are capped observations, not complete render totals.
    // Bound records and visited nodes, including text-only streaming updates.
    let budget = 256;
    const counts = { media: 0, frames: 0, answers: 0 };
    for (const record of records.slice(0, 32)) {
      for (const root of record.addedNodes) {
        const walker = document.createTreeWalker(root, NodeFilter.SHOW_ALL);
        let node: Node | null = root;
        while (node && budget > 0) {
          budget -= 1;
          if (node instanceof Element) {
            counts.media += Number(node.matches('video, audio, img'));
            counts.frames += Number(node.matches('iframe'));
            counts.answers += Number(node.matches('.question-body'));
            if (node.tagName === 'VIDEO' || node.tagName === 'AUDIO' || node.tagName === 'IFRAME' || largeImage(node)) captureMount();
            if (!transcriptSeen && node.classList.contains('chat-exchange') && node.getAttribute('data-thread-id') === focusedThreadId.peek() && node.closest('.thread-content')) {
              transcriptSeen = true;
              mark('transcript');
              captureMount();
            }
          }
          node = budget > 0 ? walker.nextNode() : null;
        }
        if (!budget) break;
      }
      if (!budget) break;
    }
    if (counts.media) mark('media-mount', { count: counts.media });
    if (counts.frames) mark('iframe-mount', { count: counts.frames });
    if (counts.answers) mark('answer-card-render', { count: counts.answers });
  });
  observer.observe(document.body, { childList: true, subtree: true });
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ['data-lucidos-event-stream'] });
  // An image may acquire its decoded dimensions only after the mount callback.
  const onLoad = (event: Event) => {
    if (event.target instanceof HTMLIFrameElement) mark('iframe-load');
    if (event.target instanceof Element && largeImage(event.target)) captureMount();
  };
  document.addEventListener('load', onLoad, true);
  const stopWake = onPageWake(() => {
    wakeUntil = performance.now() + WAKE_WINDOW_MS;
    clearTimeout(timer);
    clearTimeout(mountTimer);
    mountTimer = undefined;
    transcriptSeen = false;
    mark('wake');
    schedule();
    if (document.visibilityState === 'visible') capture();
  });
  const stopHide = onPageHide(() => { mark('hide', { state: 0, count: requestsInFlight().count }); capture(); });
  const stopPageHide = onCrashPageHide(state => mark('hide', { state, count: requestsInFlight().count }));
  const stopWorker = onCrashWorker(state => mark('worker', { state }));
  capture();
  let lastStall = -Infinity;
  return {
    stall(lag) {
      if (lag < 2000 || performance.now() - lastStall < 30_000) return;
      lastStall = performance.now();
      const data = { ...sample(), lagMs: Math.round(lag) };
      store.add('stall', data);
      store.flush();
      postClientLog('page-health', 'stall', data);
    },
    stop() {
      stopFocus();
      stopAdmission();
      observer.disconnect();
      clearTimeout(timer);
      clearTimeout(mountTimer);
      document.removeEventListener('load', onLoad, true);
      stopWake();
      stopHide();
      stopPageHide();
      stopWorker();
    },
  };
}
