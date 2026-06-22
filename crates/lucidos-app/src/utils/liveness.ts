/** Diagnostic-only telemetry to detect iOS WKWebView content-process crashes
 *  that don't fire pagehide/beforeunload. Writes a heartbeat to localStorage
 *  every few seconds while visible; on startup, computes how long the prior
 *  page was actually dead by anchoring on `performance.timeOrigin` (the new
 *  navigation's start time) rather than `Date.now()` — that strips out the
 *  ~10–20s cold-reload cost on iOS PWA, which would otherwise bury every
 *  mid-use kill above the 7s crash threshold and mislabel it as `bg_resume`.
 *
 *  Reads as engine.log lines: `[Client/lifecycle] startup {...}`. Grep for
 *  `"kind":"likely_crash"` to count crash recoveries.
 *
 *  TODO: remove after the iOS-PWA blackout investigation is closed. */

import { focusedThreadId } from '../store/store';
import { composeDrafts } from '../store/composeDrafts';
import { API } from '../api/client';
import { isIOSPwa } from './platform';

const HEARTBEAT_KEY = 'lucidos-liveness-heartbeat';
const CLEAN_CLOSE_KEY = 'lucidos-liveness-clean-close';
const HEARTBEAT_INTERVAL_MS = 3_000;
// 2x interval + jitter — gaps within this window mean the prior session was
// alive within the last tick, so an interrupted close was a crash, not bg.
const CRASH_GAP_THRESHOLD_MS = 7_000;
// Beyond this, the prior session was idle long enough that the heartbeat
// naturally stopped, so we can't distinguish crash from real cold start.
const COLD_GAP_THRESHOLD_MS = 30 * 60 * 1_000;

interface HeartbeatPayload {
  ts: number;
  focusedThread: string | null;
  draftCount: number;
}

type StartupKind =
  | 'cold'           // No prior heartbeat or > 30 min dead
  | 'bg_resume'      // Long dead window, no clean-close marker (background timed out the heartbeat)
  | 'reload_clean'   // Clean close marker present and nav type is reload
  | 'nav_clean'      // Clean close marker present and nav type is navigate / back-forward
  | 'likely_crash';  // Short dead window, no clean-close marker, iOS PWA — the bug we hunt

interface ClassifyInput {
  /** Time the prior page was actually unresponsive: navigation-start of the
   *  new page minus the last heartbeat ts. Subtracts out cold-reload duration
   *  so the 7s crash window catches WKWebView kills even when the rehydrate
   *  itself takes 10–20s on iOS PWA. */
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
    /* localStorage full / disabled — telemetry must never break the app */
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

/** When the new page's navigation began — ~when the prior page died for a
 *  crash-reload. Falls back to Date.now() so an env without performance.timeOrigin
 *  degrades to the old wallclock-at-startup behavior instead of breaking. */
function getNavigationStartMs(): number {
  try {
    if (typeof performance !== 'undefined' && typeof performance.timeOrigin === 'number' && performance.timeOrigin > 0) {
      return performance.timeOrigin;
    }
  } catch {
    /* ignore */
  }
  return Date.now();
}

/** Fire-and-forget POST to the engine's client-log breadcrumb endpoint. */
export function postClientLog(category: string, message: string, data: Record<string, unknown>): void {
  try {
    fetch(`${API}/internal/client-log`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ category, message, data }),
      keepalive: true,
    }).catch(() => { /* fire-and-forget */ });
  } catch {
    /* telemetry must never break the app */
  }
}

export function reportStartupKind(): void {
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
    /* ignore */
  }

  // gapMs = wallclock-at-mount minus last-heartbeat. Includes the cold-reload
  // duration (~10–20s on iOS PWA), so it overshoots the 7s crash threshold even
  // when the prior page was killed mid-use. Kept in the payload for continuity
  // with historical log lines.
  //
  // deadMs = navigation-start minus last-heartbeat. Approximates the actual
  // dead-window the user perceived: it strips out the cold-reload cost, so a
  // WKWebView content-process kill in a fresh page surfaces as deadMs ≈ 0–3s
  // and the classifier can correctly label it likely_crash.
  const navStart = getNavigationStartMs();
  const gapMs = prior ? Date.now() - prior.ts : null;
  const deadMs = prior ? Math.max(0, navStart - prior.ts) : null;
  const reloadMs = gapMs !== null && deadMs !== null ? Math.max(0, gapMs - deadMs) : null;
  const navType = getNavType();
  const isPwa = isIOSPwa();
  const kind = classifyStartup({ deadMs, cleanClose, navType, isIOSPwa: isPwa });

  // Capture the landing URL's deep-link channels (query + hash) so a push tap
  // that "opens the app but doesn't navigate" is diagnosable: this is the only
  // breadcrumb that fires on EVERY boot, including the iOS declarative-push
  // reload. If `search`/`hash` carry `notification=…&thread=…&tap=…` the deep
  // link reached the page (look downstream for why nav was lost); if they're
  // empty the params were dropped before boot (scope → picker, or a redirect).
  let landingSearch = '';
  let landingHash = '';
  let landingPath = '';
  try {
    landingSearch = window.location.search.slice(0, 200);
    landingHash = window.location.hash.slice(0, 200);
    landingPath = window.location.pathname.slice(0, 80);
  } catch {
    /* telemetry only */
  }
  postClientLog('lifecycle', 'startup', {
    kind,
    gap_ms: gapMs,
    dead_ms: deadMs,
    reload_ms: reloadMs,
    clean_close: cleanClose,
    nav_type: navType,
    is_ios_pwa: isPwa,
    ua: typeof navigator !== 'undefined' ? navigator.userAgent.slice(0, 100) : '',
    prev_focused_thread: prior?.focusedThread ?? null,
    prev_draft_count: prior?.draftCount ?? 0,
    landing_path: landingPath,
    landing_search: landingSearch,
    landing_hash: landingHash,
  });
}

// ---------------------------------------------------------------------------
// Uncaught error / rejection breadcrumbs
// ---------------------------------------------------------------------------
// Same channel as startup classification: every iOS PWA black-screen recovery
// today logs `kind:"reload_clean"` with no JS trace, so the cause is invisible.
// Capture window-level errors and unhandled promise rejections to the engine
// log so the next blackout leaves evidence (engine.log line tagged
// `[Client/error]`). Throttled per error signature so a hot loop can't drown
// out the log tail.

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
    // Always coerce to a real string — JSON.stringify returns undefined for
    // symbol / function / undefined reasons (no throw), and the subsequent
    // .slice would crash the rejection handler itself.
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

  function markCleanClose() {
    try {
      localStorage.setItem(CLEAN_CLOSE_KEY, '1');
    } catch {
      /* ignore */
    }
  }

  window.addEventListener('beforeunload', markCleanClose);
  window.addEventListener('pagehide', markCleanClose);

  const stopErrorBreadcrumbs = installErrorBreadcrumbs();

  return () => {
    clearInterval(intervalId);
    window.removeEventListener('beforeunload', markCleanClose);
    window.removeEventListener('pagehide', markCleanClose);
    stopErrorBreadcrumbs();
  };
}
