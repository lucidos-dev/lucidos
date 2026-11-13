import { watchPageAway } from './pageVisit';

/** Fetch-path probe for the page's service worker. It targets a worker whose
 *  event loop runs but whose own `fetch()` never settles (seen on an iOS PWA
 *  after a notification tap). Such a worker answers `swLivenessProbe`'s ping, yet
 *  every `GET /api/v1/*` it forwards hangs, so the page shows a skeleton forever.
 *
 *  The probe asks `/api/v1/health` twice at once. The plain request skips the
 *  worker. The marked request goes through the worker's `fetch()`. The plain one
 *  answering while the marked one hangs is firm evidence of a wedge. */

/** Keep in step with FETCH_PROBE_PARAM in public/sw.js. */
export const FETCH_PROBE_PARAM = 'sw-fetch-probe';
/** Two windows must end before the boot splash's 15s cap, or the user gives up first. */
export const SW_FETCH_PROBE_TIMEOUT_MS = 4000;
/** Caps automatic recovery to one reload per window, so an unfixable wedge never loops. */
export const SW_FETCH_RECOVERY_WINDOW_MS = 60_000;
const RECOVERY_KEY = 'lucidos-sw-fetch-recovery-at';

/** Claims the one automatic recovery of this window. Returns false when
 *  sessionStorage is unavailable: with no guard, reloading risks a loop. */
export function claimFetchPathRecovery(now: number): boolean {
  try {
    const last = Number(sessionStorage.getItem(RECOVERY_KEY) || '0');
    if (now - last < SW_FETCH_RECOVERY_WINDOW_MS) return false;
    sessionStorage.setItem(RECOVERY_KEY, String(now));
    return true;
  } catch {
    return false;
  }
}

export function fetchProbeUrls(healthUrl: string): { direct: string; forwarded: string } {
  return { direct: healthUrl, forwarded: `${healthUrl}?${FETCH_PROBE_PARAM}=1` };
}

export interface SwFetchPathProbe {
  check: () => Promise<void>;
  stop: () => void;
}

export function createSwFetchPathProbe(deps: {
  /** True when a worker controls the page. */
  hasController: () => boolean;
  /** Fetches a URL. A response of any status counts as answered. */
  fetchUrl: (url: string, signal: AbortSignal) => Promise<unknown>;
  healthUrl: string;
  /** Resets the worker and reloads the page. */
  recover: () => Promise<void>;
  /** The first window missed. Logged so a wedge the second window clears still shows. */
  onSuspected: () => void;
  onWedged: () => void;
}): SwFetchPathProbe {
  let inFlight = false;
  let stopped = false;

  /** True only on firm evidence: the plain probe answered, the forwarded one is
   *  still silent at the deadline, and the page was awake throughout. */
  async function forwardedPathIsWedged(): Promise<boolean> {
    const away = watchPageAway();
    const urls = fetchProbeUrls(deps.healthUrl);
    const abort = new AbortController();
    let directAnswered = false;
    let ruledOut = false;
    let finish: () => void = () => {};
    const ended = new Promise<void>((resolve) => { finish = resolve; });
    // A forwarded request that settles, even in error, is no hang. A failed
    // plain request means the network is down, which proves nothing about the worker.
    deps.fetchUrl(urls.forwarded, abort.signal).catch(() => {}).then(() => { ruledOut = true; finish(); });
    deps.fetchUrl(urls.direct, abort.signal).then(
      () => { directAnswered = true; },
      () => { ruledOut = true; finish(); },
    );
    const timer = setTimeout(finish, SW_FETCH_PROBE_TIMEOUT_MS);
    await ended;
    clearTimeout(timer);
    abort.abort();
    // A hidden page can be frozen, and its overdue timer then beats the queued answers.
    return directAnswered && !ruledOut && !away();
  }

  async function check() {
    if (inFlight || stopped || !deps.hasController()) return;
    inFlight = true;
    try {
      // A worker cold-starting after a wake can miss one window. A wedged one misses both.
      if (!(await forwardedPathIsWedged()) || stopped) return;
      deps.onSuspected();
      if (!(await forwardedPathIsWedged()) || stopped) return;
      if (!claimFetchPathRecovery(Date.now())) return;
      deps.onWedged();
      await deps.recover();
    } finally {
      inFlight = false;
    }
  }

  return {
    check,
    stop() {
      stopped = true;
    },
  };
}
