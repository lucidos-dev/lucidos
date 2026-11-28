import { watchPageAway } from './pageVisit';


export const SW_PROBE_TIMEOUT_MS = 5000;
export const SW_RECOVERY_COOLDOWN_MS = 60_000;

type ProbeResult = 'answered' | 'silent' | 'no-evidence';

export interface SwLivenessProbe {
  check: () => Promise<void>;
  notePong: (instance?: unknown) => void;
  stop: () => void;
}

export function createSwLivenessProbe(deps: {
  ping: () => boolean;
  recover: () => Promise<void>;
  onRecovered: () => void;
  onState?: (state: number) => void;
}): SwLivenessProbe {
  let lastPongAt = 0;
  let workerInstance: number | undefined;
  let lastRecoveryAt = 0;
  let probeInFlight = false;
  let stopped = false;
  let pongResolver: (() => void) | null = null;

  async function probeOnce(): Promise<ProbeResult> {
    const sentAt = Date.now();
    const away = watchPageAway();
    if (!deps.ping()) return 'no-evidence';
    deps.onState?.(0);
    let timeoutHandle: ReturnType<typeof setTimeout> | null = null;
    await new Promise<void>((resolve) => {
      pongResolver = () => { if (timeoutHandle !== null) clearTimeout(timeoutHandle); resolve(); };
      timeoutHandle = setTimeout(() => { timeoutHandle = null; resolve(); }, SW_PROBE_TIMEOUT_MS);
    });
    pongResolver = null;
    if (lastPongAt >= sentAt) return 'answered';
    if (away()) return 'no-evidence';
    return 'silent';
  }

  async function check() {
    if (probeInFlight || stopped) return;
    if (Date.now() - lastRecoveryAt < SW_RECOVERY_COOLDOWN_MS) return;
    probeInFlight = true;
    try {
      for (let attempt = 0; attempt < 2; attempt++) {
        const result = await probeOnce();
        if (stopped || result !== 'silent') return;
      }
      lastRecoveryAt = Date.now();
      await deps.recover();
      if (!stopped) deps.onRecovered();
    } finally {
      probeInFlight = false;
    }
  }

  return {
    check,
    notePong(instance) {
      const valid = typeof instance === 'number' && Number.isFinite(instance) && instance >= 0;
      deps.onState?.(valid && workerInstance !== undefined && workerInstance !== instance ? 2 : 1);
      if (valid) workerInstance = instance;
      lastPongAt = Date.now();
      pongResolver?.();
    },
    stop() {
      stopped = true;
    },
  };
}
