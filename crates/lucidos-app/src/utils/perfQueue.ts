
import { API } from './basePath';
import { enginePackaged } from '../store/enginePackaging';
import { CLIENT_LOG_MAX_BATCH } from '@lucidos/engine-constants';

interface PerfSample {
  message: string;
  data: Record<string, unknown>;
}

const FLUSH_SIZE = 20;
const FLUSH_INTERVAL_MS = 10_000;

let buffer: PerfSample[] = [];
let timer: ReturnType<typeof setInterval> | null = null;

export const PERF_FLAG_KEY = 'lucidos:perf';
let perfEnabledOverride: boolean | null = null;

export function isPerfEnabled(): boolean {
  try {
    return typeof localStorage !== 'undefined' && localStorage.getItem(PERF_FLAG_KEY) === '1';
  } catch {
    return false;
  }
}

const gateListeners = new Set<(on: boolean) => void>();

export function onPerfEnabledChange(listener: (on: boolean) => void): () => void {
  gateListeners.add(listener);
  return () => { gateListeners.delete(listener); };
}

const optInListeners = new Set<(on: boolean) => void>();

export function onPerfOptInChange(listener: (on: boolean) => void): () => void {
  optInListeners.add(listener);
  return () => { optInListeners.delete(listener); };
}

export function setPerfEnabled(on: boolean): void {
  if (enginePackaged.peek() !== true) return;
  const previous = isPerfEnabled();
  try {
    if (typeof localStorage !== 'undefined') {
      if (on) localStorage.setItem(PERF_FLAG_KEY, '1');
      else localStorage.removeItem(PERF_FLAG_KEY);
    }
  } catch {
  }
  perfRecordingOn();
  const current = isPerfEnabled();
  if (current !== previous) notifyListeners(optInListeners, current);
}

function notifyListeners(listeners: Set<(on: boolean) => void>, on: boolean): void {
  for (const listener of listeners) {
    try {
      listener(on);
    } catch {
    }
  }
}

export function perfRecordingOn(): boolean {
  const packaged = enginePackaged.peek();
  const on = perfEnabledOverride ?? (packaged === false || (packaged === true && isPerfEnabled()));
  if (announcedGate !== on) {
    announcedGate = on;
    notifyListeners(gateListeners, on);
  }
  return on;
}

let announcedGate: boolean | null = null;

enginePackaged.subscribe(() => perfRecordingOn());

export function _setPerfEnabledForTesting(value: boolean | null): void {
  perfEnabledOverride = value;
  perfRecordingOn();
}

export function trimToCap<T>(buf: T[], cap: number): T[] {
  return buf.length > cap ? buf.slice(buf.length - cap) : buf;
}

export function _resetPerfQueueForTesting(): void {
  announcedGate = null;
  buffer = [];
  if (timer !== null) {
    clearInterval(timer);
    timer = null;
  }
}

function ensureTimer(): void {
  if (timer !== null || typeof setInterval === 'undefined') return;
  timer = setInterval(() => flushPerfQueue(), FLUSH_INTERVAL_MS);
}

export function recordPerfSample(message: string, data: Record<string, unknown>): void {
  if (!perfRecordingOn()) return;
  try {
    buffer.push({ message, data });
    if (buffer.length > CLIENT_LOG_MAX_BATCH) buffer = trimToCap(buffer, CLIENT_LOG_MAX_BATCH);
    ensureTimer();
    if (buffer.length >= FLUSH_SIZE) flushPerfQueue();
  } catch {
  }
}

function requeueFailed(failed: PerfSample[]): void {
  buffer = trimToCap([...failed, ...buffer], CLIENT_LOG_MAX_BATCH);
}

export function flushPerfQueue(keepalive = false): void {
  if (buffer.length === 0) return;
  const batch = buffer;
  buffer = [];
  const samples = batch.map((s) => ({ category: 'perf', code: s.message, message: s.message, data: s.data }));
  try {
    fetch(`${API}/internal/client-logs`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(samples),
      keepalive,
    })
      .catch(() => requeueFailed(batch));
  } catch {
    requeueFailed(batch);
  }
}

if (typeof document !== 'undefined') {
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'hidden') flushPerfQueue(true);
  });
}
if (typeof window !== 'undefined') {
  window.addEventListener('pagehide', () => flushPerfQueue(true));
  window.addEventListener('storage', (event) => {
    if (event.key !== PERF_FLAG_KEY && event.key !== null) return;
    try {
      if (event.storageArea !== window.localStorage) return;
    } catch {
      return;
    }
    if (event.key === null || (event.oldValue === '1') !== (event.newValue === '1')) {
      notifyListeners(optInListeners, isPerfEnabled());
    }
  });
}
