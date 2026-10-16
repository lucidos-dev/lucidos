import { watchPageAway } from './pageVisit';

/** Liveness probe for the page's service worker. It targets Chrome's wedged
 *  worker (Chromium issue #370536109), which answers no message at all. The
 *  repair unregisters the worker and resubscribes push, so a false alarm costs
 *  the user a push endpoint. A probe therefore repairs only on firm evidence:
 *  two silent windows in a row, with the page awake through both. */

export const SW_PROBE_TIMEOUT_MS = 5000;
/** Caps the repair rate, so a misbehaving worker cannot churn push endpoints. */
export const SW_RECOVERY_COOLDOWN_MS = 60_000;

type ProbeResult = 'answered' | 'silent' | 'no-evidence';

export interface SwLivenessProbe {
  check: () => Promise<void>;
  notePong: () => void;
  stop: () => void;
}

export function createSwLivenessProbe(deps: {
  /** Pings the controlling worker. Returns false when no worker controls the page. */
  ping: () => boolean;
  recover: () => Promise<void>;
  onRecovered: () => void;
}): SwLivenessProbe {
  let lastPongAt = 0;
  let lastRecoveryAt = 0;
  let probeInFlight = false;
  let stopped = false;
  let pongResolver: (() => void) | null = null;

  async function probeOnce(): Promise<ProbeResult> {
    const sentAt = Date.now();
    const away = watchPageAway();
    if (!deps.ping()) return 'no-evidence';
    let timeoutHandle: ReturnType<typeof setTimeout> | null = null;
    await new Promise<void>((resolve) => {
      pongResolver = () => { if (timeoutHandle !== null) clearTimeout(timeoutHandle); resolve(); };
      timeoutHandle = setTimeout(() => { timeoutHandle = null; resolve(); }, SW_PROBE_TIMEOUT_MS);
    });
    pongResolver = null;
    if (lastPongAt >= sentAt) return 'answered';
    // A hidden page can be frozen. On wake its overdue timeout can run before
    // the pong that sat queued meanwhile, so the silence proves nothing.
    if (away()) return 'no-evidence';
    return 'silent';
  }

  async function check() {
    if (probeInFlight || stopped) return;
    if (Date.now() - lastRecoveryAt < SW_RECOVERY_COOLDOWN_MS) return;
    probeInFlight = true;
    try {
      // A worker cold-starting after a wake can miss one window. A wedged one
      // misses every window, so a second silent probe confirms it.
      for (let attempt = 0; attempt < 2; attempt++) {
        const result = await probeOnce();
        if (stopped || result !== 'silent') return;
      }
      lastRecoveryAt = Date.now();
      // A throw bubbles to the caller. The repair toast fires only on success,
      // and a lasting wedge resurfaces on the next probe after the cooldown.
      await deps.recover();
      if (!stopped) deps.onRecovered();
    } finally {
      probeInFlight = false;
    }
  }

  return {
    check,
    notePong() {
      lastPongAt = Date.now();
      pongResolver?.();
    },
    stop() {
      stopped = true;
    },
  };
}
