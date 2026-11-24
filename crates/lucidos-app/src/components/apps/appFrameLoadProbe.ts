/**
 * Says why an app frame sat on its load cover. After `SLOW_LOAD_MS`, a frame
 * that is still covered or unloaded writes one `[Client/app-frame]` line with
 * what the host can see. It writes one more when the frame finishes or goes
 * away. A frame that loads in time writes nothing.
 *
 * The host is the only reliable observer: a frame whose `sdk.js` never arrived
 * runs no telemetry of its own. The probe only reads. It never reloads, reveals
 * or requests anything for the frame. A temporary measure: see
 * `docs/temporary-measures.md` § App frame load probe.
 */
import { postClientLog } from '../../utils/clientLog';
import { sinceResumeMs } from '../../utils/resumeClock';
import { requestsInFlight, type RequestsInFlight } from '../../api/client/requestsInFlight';
import { bridgedFetchSnapshot } from '../../store/actions/app-bridge';
import type { LanesSnapshot } from '../../store/actions/bridgedFetchLanes';

export const SLOW_LOAD_MS = 3000;

const CATEGORY = 'app-frame';

export interface FrameLabel {
  app: string;
  place: string;
}

export interface HostSample {
  sinceResumeMs: number | null;
  eventStream: string | null;
  lanes: LanesSnapshot;
  requests: RequestsInFlight;
}

export function sampleHost(): HostSample {
  return {
    sinceResumeMs: sinceResumeMs(),
    eventStream: document.documentElement.dataset.lucidosEventStream ?? null,
    lanes: bridgedFetchSnapshot(),
    requests: requestsInFlight(),
  };
}

export type FrameDocumentTiming = Record<
  'start' | 'fetchStart' | 'requestStart' | 'responseStart' | 'responseEnd',
  number | null
>;

export function frameDocumentTiming(
  frame: HTMLIFrameElement | null,
  attachedAt: number,
): FrameDocumentTiming | null {
  if (!frame?.src) return null;
  const url = frame.src.split('#', 1)[0];
  let own: PerformanceResourceTiming[];
  try {
    own = (performance.getEntriesByName(url, 'resource') as PerformanceResourceTiming[])
      .filter((entry) => entry.startTime >= attachedAt);
  } catch {
    return null;
  }
  if (own.length !== 1) return null;
  const [entry] = own;
  const ms = (value: number) => (value > 0 ? Math.round(value - attachedAt) : null);
  return {
    start: ms(entry.startTime),
    fetchStart: ms(entry.fetchStart),
    requestStart: ms(entry.requestStart),
    responseStart: ms(entry.responseStart),
    responseEnd: ms(entry.responseEnd),
  };
}

export interface FrameLoadProbe {
  noteLoad(): void;
  noteReady(): void;
  noteRevealed(fused: boolean): void;
  abandon(): void;
}

type Log = (category: string, message: string, data: Record<string, unknown>) => void;

export function startFrameLoadProbe(
  label: FrameLabel,
  frame: () => HTMLIFrameElement | null,
  attachedAt: number,
  host: () => HostSample = sampleHost,
  log: Log = postClientLog,
): FrameLoadProbe {
  const mountedAt = Date.now();
  const since = () => Date.now() - mountedAt;
  let loadMs: number | null = null;
  let readyMs: number | null = null;
  let revealMs: number | null = null;
  let fused = false;
  let slow = false;
  let closed = false;

  const times = () => ({ loadMs, readyMs, revealMs, fused });
  const documentTiming = () => ({
    frameDocument: loadMs === null ? null : frameDocumentTiming(frame(), attachedAt),
  });
  const end = (message: string) =>
    log(CATEGORY, message, { ...label, sinceMountMs: since(), ...times(), ...documentTiming() });
  const settleIfDone = () => {
    if (closed || loadMs === null || revealMs === null) return;
    closed = true;
    clearTimeout(timer);
    if (slow) end('slow load ended');
  };

  const timer = setTimeout(() => {
    slow = true;
    log(CATEGORY, 'slow load', {
      ...label,
      sinceMountMs: since(),
      ...times(),
      loaded: loadMs !== null,
      ready: readyMs !== null,
      ...host(),
      ...documentTiming(),
    });
  }, SLOW_LOAD_MS);

  return {
    noteLoad() {
      loadMs ??= since();
      settleIfDone();
    },
    noteReady() {
      readyMs ??= since();
    },
    noteRevealed(byFuse) {
      if (revealMs !== null) return;
      revealMs = since();
      fused = byFuse;
      settleIfDone();
    },
    abandon() {
      if (closed) return;
      closed = true;
      clearTimeout(timer);
      if (slow) end('slow load abandoned');
    },
  };
}
