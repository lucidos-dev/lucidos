/** The shell's own engine requests that have not answered yet. The app frame
 *  load probe reads it, to see whether the connection is stalled for the shell
 *  as well as for the frames. A request counts until its response headers
 *  arrive. A temporary measure: see `docs/temporary-measures.md` § App frame
 *  load probe. */

const MAX_PATHS = 3;
const MAX_PATH_CHARS = 80;

interface Pending {
  path: string;
  startedAt: number;
}

const pending = new Set<Pending>();

export interface RequestsInFlight {
  count: number;
  oldestMs: number | null;
  /** The paths of the oldest few, oldest first, without their query. */
  oldestPaths: string[];
}

function pathOf(url: string): string {
  try {
    const { pathname } = new URL(url, window.location.origin);
    return pathname.slice(0, MAX_PATH_CHARS);
  } catch {
    return url.slice(0, MAX_PATH_CHARS);
  }
}

/** Count a request from now until the returned function runs. */
export function trackRequest(url: string): () => void {
  const entry: Pending = { path: pathOf(url), startedAt: Date.now() };
  pending.add(entry);
  return () => { pending.delete(entry); };
}

export function requestsInFlight(now = Date.now()): RequestsInFlight {
  const oldest = [...pending].sort((a, b) => a.startedAt - b.startedAt);
  return {
    count: oldest.length,
    oldestMs: oldest.length > 0 ? now - oldest[0].startedAt : null,
    oldestPaths: oldest.slice(0, MAX_PATHS).map((p) => p.path),
  };
}
