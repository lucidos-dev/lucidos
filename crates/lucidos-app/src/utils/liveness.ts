/** Client-stability debug tooling — a fire-and-forget breadcrumb channel
 *  (`postClientLog`, shared by many call sites) plus iOS-PWA liveness telemetry:
 *  a heartbeat + startup classification + uncaught-error breadcrumbs that detect
 *  iOS WKWebView content-process crashes (which don't fire pagehide/beforeunload).
 *  Reads as engine.log `[Client/lifecycle]` / `[Client/error]` lines; grep
 *  `"kind":"likely_crash"` to count crash recoveries.
 *
 *  PERMANENT debug tooling (not a temporary measure). Built for the
 *  `ios-pwa-blackout` investigation (resolved 2026-06-30 — see
 *  docs/temporary-measures.md), but RETAINED rather than deleted: the breadcrumb
 *  channel is shared infrastructure and the crash / error telemetry is reusable
 *  for any future client-stability issue. */

import { CLIENT_BUILD_ID } from 'virtual:build-id';
import { formatBuildId } from './buildId';
import { focusedThreadId } from '../store/store';
import { composeDrafts } from '../store/composeDrafts';
import { postClientLog } from './clientLog';
import { isIOSPwa } from './platform';
import { onPageHide, onPageWake } from './pageVisit';
import { noteCrashPageHide } from './crashPageHide';
import { takeCrashRecord } from './crashStorage';

const HEARTBEAT_KEY = 'lucidos-liveness-heartbeat';
const CLEAN_CLOSE_KEY = 'lucidos-liveness-clean-close';
const HEARTBEAT_INTERVAL_MS = 3_000;
const CRASH_GAP_THRESHOLD_MS = 7_000;
const COLD_GAP_THRESHOLD_MS = 30 * 60 * 1_000;

interface HeartbeatPayload {
  ts: number;
  hiddenAt?: number;
  focusedThread: string | null;
  draftCount: number;
}

type StartupKind =
  | 'cold'
  | 'bg_resume'
  | 'reload_clean'
  | 'nav_clean'
  | 'likely_crash';

interface ClassifyInput {
  deadMs: number | null;
  cleanClose: boolean;
  navType: string;
  isIOSPwa: boolean;
}

export function classifyStartup(input: ClassifyInput): StartupKind {
  if (input.deadMs === null || input.deadMs > COLD_GAP_THRESHOLD_MS) {
    return 'cold';
  }
  if (input.cleanClose) {
    return input.navType === 'reload' ? 'reload_clean' : 'nav_clean';
  }
  if (input.deadMs <= CRASH_GAP_THRESHOLD_MS && input.isIOSPwa) {
    return 'likely_crash';
  }
  return 'bg_resume';
}

function readPriorHeartbeat(): HeartbeatPayload | null {
  try {
    const raw = localStorage.getItem(HEARTBEAT_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as HeartbeatPayload | null;
    if (!parsed || typeof parsed.ts !== 'number') return null;
    return parsed;
  } catch {
    return null;
  }
}

function writeHeartbeat(): void {
  const payload: HeartbeatPayload = {
    ts: Date.now(),
    focusedThread: focusedThreadId.value,
    draftCount: composeDrafts.value.size,
  };
  try {
    localStorage.setItem(HEARTBEAT_KEY, JSON.stringify(payload));
  } catch {
  }
}

function getNavType(): string {
  try {
    const entries = performance.getEntriesByType('navigation');
    const nav = entries[0] as PerformanceNavigationTiming | undefined;
    return nav?.type ?? 'unknown';
  } catch {
    return 'unknown';
  }
}

function getNavigationStartMs(): number {
  try {
    if (typeof performance !== 'undefined' && typeof performance.timeOrigin === 'number' && performance.timeOrigin > 0) {
      return performance.timeOrigin;
    }
  } catch {
  }
  return Date.now();
}

export { postClientLog };

export function reportStartupKind(): void | Promise<void> {
  const prior = readPriorHeartbeat();
  const cleanClose = (() => {
    try {
      return localStorage.getItem(CLEAN_CLOSE_KEY) === '1';
    } catch {
      return false;
    }
  })();
  try {
    localStorage.removeItem(CLEAN_CLOSE_KEY);
  } catch {
  }

  const navStart = getNavigationStartMs();
  const gapMs = prior ? Date.now() - prior.ts : null;
  const deadMs = prior ? Math.max(0, navStart - prior.ts) : null;
  const reloadMs = gapMs !== null && deadMs !== null ? Math.max(0, gapMs - deadMs) : null;
  const navType = getNavType();
  const isPwa = isIOSPwa();
  const kind = classifyStartup({ deadMs, cleanClose, navType, isIOSPwa: isPwa });

  let landingSearch = '';
  let landingHash = '';
  let landingPath = '';
  try {
    landingSearch = window.location.search.slice(0, 200);
    landingHash = window.location.hash.slice(0, 200);
    landingPath = window.location.pathname.slice(0, 80);
  } catch {
  }
  const raw = takeCrashRecord();
  const data = {
    build: formatBuildId(CLIENT_BUILD_ID),
    kind,
    gap_ms: gapMs,
    dead_ms: deadMs,
    reload_ms: reloadMs,
    prev_hidden_ms: prior?.hiddenAt !== undefined && Number.isFinite(prior.hiddenAt)
      ? Math.max(0, navStart - prior.hiddenAt) : null,
    clean_close: cleanClose,
    nav_type: navType,
    is_ios_pwa: isPwa,
    ua: typeof navigator !== 'undefined' ? navigator.userAgent.slice(0, 100) : '',
    prev_focused_thread: prior?.focusedThread ?? null,
    prev_draft_count: prior?.draftCount ?? 0,
    landing_path: landingPath,
    landing_search: landingSearch,
    landing_hash: landingHash,
  };
  if (kind === 'likely_crash' && raw) {
    return import('./crashBreadcrumbs').then(({ recoveredStartup, BreadcrumbStore }) => {
      const store = new BreadcrumbStore({ getItem: () => raw, setItem: () => {} }, '');
      postClientLog('lifecycle', 'startup', recoveredStartup(data, store));
    }, () => { postClientLog('lifecycle', 'startup', data); });
  }
  postClientLog('lifecycle', 'startup', data);
}


const ERROR_THROTTLE_MS = 30_000;
const ERROR_THROTTLE_MAX_KEYS = 100;
const errorLastSeen = new Map<string, number>();

function shouldEmitError(key: string): boolean {
  const now = Date.now();
  const last = errorLastSeen.get(key) ?? 0;
  if (now - last < ERROR_THROTTLE_MS) return false;
  errorLastSeen.set(key, now);
  if (errorLastSeen.size > ERROR_THROTTLE_MAX_KEYS) {
    for (const [k, t] of errorLastSeen) {
      if (now - t > ERROR_THROTTLE_MS) errorLastSeen.delete(k);
    }
  }
  return true;
}

function safeUrl(): string {
  try {
    return window.location.href.slice(0, 200);
  } catch {
    return '';
  }
}

function installErrorBreadcrumbs(): () => void {
  function onError(e: ErrorEvent) {
    const msg = e.message || 'unknown error';
    const src = e.filename || '';
    const key = `e:${msg}:${src}:${e.lineno ?? 0}`;
    if (!shouldEmitError(key)) return;
    const stack = e.error instanceof Error ? e.error.stack ?? null : null;
    postClientLog('error', 'uncaught', {
      message: msg.slice(0, 200),
      source: src.slice(0, 200),
      line: e.lineno ?? null,
      col: e.colno ?? null,
      stack: stack ? stack.slice(0, 1000) : null,
      is_ios_pwa: isIOSPwa(),
      url: safeUrl(),
      ua: typeof navigator !== 'undefined' ? navigator.userAgent.slice(0, 100) : '',
    });
  }
  function onRejection(e: PromiseRejectionEvent) {
    const reason = e.reason;
    const isError = reason instanceof Error;
    const msg = typeof reason === 'string'
      ? reason
      : isError
        ? reason.message
        : ((): string => {
            try {
              return JSON.stringify(reason) ?? String(reason);
            } catch {
              return String(reason);
            }
          })();
    const key = `r:${msg.slice(0, 100)}`;
    if (!shouldEmitError(key)) return;
    postClientLog('error', 'unhandledrejection', {
      message: msg.slice(0, 200),
      stack: isError ? reason.stack?.slice(0, 1000) ?? null : null,
      is_ios_pwa: isIOSPwa(),
      url: safeUrl(),
      ua: typeof navigator !== 'undefined' ? navigator.userAgent.slice(0, 100) : '',
    });
  }
  window.addEventListener('error', onError);
  window.addEventListener('unhandledrejection', onRejection);
  return () => {
    window.removeEventListener('error', onError);
    window.removeEventListener('unhandledrejection', onRejection);
  };
}

export function startLivenessTracking(): () => void {
  writeHeartbeat();

  const intervalId = window.setInterval(() => {
    if (typeof document !== 'undefined' && document.visibilityState !== 'visible') return;
    writeHeartbeat();
  }, HEARTBEAT_INTERVAL_MS);

  const stopHide = onPageHide(() => {
    const prior = readPriorHeartbeat();
    if (!prior) return;
    try {
      localStorage.setItem(HEARTBEAT_KEY, JSON.stringify({ ...prior, hiddenAt: Date.now() }));
    } catch {  }
  });
  const stopWake = onPageWake(writeHeartbeat);
  function onHide(event: PageTransitionEvent) {
    markCleanClose();
    noteCrashPageHide(event.persisted);
  }
  function markCleanClose() {
    try {
      localStorage.setItem(CLEAN_CLOSE_KEY, '1');
    } catch {
    }
  }

  window.addEventListener('beforeunload', markCleanClose);
  window.addEventListener('pagehide', onHide);

  const stopErrorBreadcrumbs = installErrorBreadcrumbs();

  return () => {
    clearInterval(intervalId);
    window.removeEventListener('beforeunload', markCleanClose);
    window.removeEventListener('pagehide', onHide);
    stopHide();
    stopWake();
    stopErrorBreadcrumbs();
  };
}
