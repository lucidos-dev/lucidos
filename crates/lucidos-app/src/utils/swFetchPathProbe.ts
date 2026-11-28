import { watchPageAway } from './pageVisit';


export const FETCH_PROBE_PARAM = 'sw-fetch-probe';
export const SW_FETCH_PROBE_TIMEOUT_MS = 4000;
export const SW_FETCH_RECOVERY_WINDOW_MS = 60_000;
const RECOVERY_KEY = 'lucidos-sw-fetch-recovery-at';

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
  hasController: () => boolean;
  fetchUrl: (url: string, signal: AbortSignal) => Promise<unknown>;
  healthUrl: string;
  recover: () => Promise<void>;
  onSuspected: () => void;
  onWedged: () => void;
}): SwFetchPathProbe {
  let inFlight = false;
  let stopped = false;

  async function forwardedPathIsWedged(): Promise<boolean> {
    const away = watchPageAway();
    const urls = fetchProbeUrls(deps.healthUrl);
    const abort = new AbortController();
    let directAnswered = false;
    let ruledOut = false;
    let finish: () => void = () => {};
    const ended = new Promise<void>((resolve) => { finish = resolve; });
    deps.fetchUrl(urls.forwarded, abort.signal).catch(() => {}).then(() => { ruledOut = true; finish(); });
    deps.fetchUrl(urls.direct, abort.signal).then(
      () => { directAnswered = true; },
      () => { ruledOut = true; finish(); },
    );
    const timer = setTimeout(finish, SW_FETCH_PROBE_TIMEOUT_MS);
    await ended;
    clearTimeout(timer);
    abort.abort();
    return directAnswered && !ruledOut && !away();
  }

  async function check() {
    if (inFlight || stopped || !deps.hasController()) return;
    inFlight = true;
    try {
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
