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

/** Which frame this is, in the words the log is read in. */
export interface FrameLabel {
  app: string;
  /** `canvas`, or a widget's place: `dashboard`, `inline`, `shelf`, `option`. */
  place: string;
}

/** What the host knows about its own connection to the engine. */
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

/** The frame document's own request, in milliseconds from navigation start, or
 *  `null` when the browser does not expose it. */
export function frameDocumentTiming(frame: HTMLIFrameElement | null): Record<string, number> | null {
  if (!frame?.src) return null;
  const url = frame.src.split('#', 1)[0];
  const entries = performance.getEntriesByName(url, 'resource') as PerformanceResourceTiming[];
  const entry = entries[entries.length - 1];
  if (!entry) return null;
  const ms = (value: number) => Math.round(value);
  return {
    start: ms(entry.startTime),
    fetchStart: ms(entry.fetchStart),
    requestStart: ms(entry.requestStart),
    responseStart: ms(entry.responseStart),
    responseEnd: ms(entry.responseEnd),
  };
}

export interface FrameLoadProbe {
  /** The frame's document fired `load`. */
  noteLoad(): void;
  /** The app called `lucidos.ui.ready()`. */
  noteReady(): void;
  /** The cover lifted. `fused` says the fuse lifted it. */
  noteRevealed(fused: boolean): void;
  /** The frame element is going away, by unmount or by a reload. */
  abandon(): void;
}

type Log = (category: string, message: string, data: Record<string, unknown>) => void;

/** Start watching one frame element. One probe per element: a reload takes a
 *  fresh element, so it takes a fresh probe. `frame` is read when a line is
 *  written, since the element may not exist yet. */
export function startFrameLoadProbe(
  label: FrameLabel,
  frame: () => HTMLIFrameElement | null,
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
  // Loaded and revealed: nothing is left to wait for.
  const settleIfDone = () => {
    if (closed || loadMs === null || revealMs === null) return;
    closed = true;
    clearTimeout(timer);
    if (slow) log(CATEGORY, 'slow load ended', { ...label, sinceMountMs: since(), ...times() });
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
      frameDocument: frameDocumentTiming(frame()),
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
      if (slow) log(CATEGORY, 'slow load abandoned', { ...label, sinceMountMs: since(), ...times() });
    },
  };
}
