/**
 * Runs `run` over every item with at most `limit` in flight at a time, and
 * resolves once they have all settled. Never rejects because a TASK failed; a
 * non-positive `limit` is a programming error and throws at the call.
 *
 * Built for the thread-events fan-out: `loadAllThreads`' eager boot loads and
 * `runResumeSync`'s failed-load retry each issue one full snapshot request per
 * thread, which on a large workspace is dozens at once. Over HTTP/2 there is no
 * per-host connection cap, so an unbounded `Promise.all` puts all of them on one
 * connection and one radio wake, and each carries its own 10s client deadline.
 * On a phone whose link is still being re-established, the burst is what spends
 * those deadlines: the engine answers each request in single-digit milliseconds.
 * Bounding it keeps the link busy without the herd. (The wake and SSE-reconnect
 * REFRESH fan-outs were callers too, until they stopped fetching every loaded
 * thread and started marking them instead.)
 *
 * Never rejecting is the contract every call site already relied on through
 * `Promise.all(...).catch(() => {})`: a per-item failure is handled inside the
 * item's own function (a toast, a `Loadable`, a flag), and one bad item must not
 * cancel the rest of a resync. A rejection here would also strand the pool with
 * work still queued.
 */
export async function runWithConcurrency<T>(
  items: readonly T[],
  limit: number,
  run: (item: T) => Promise<unknown>,
): Promise<void> {
  if (limit <= 0) {
    throw new Error(`runWithConcurrency: limit must be > 0, got ${limit}`);
  }
  if (items.length === 0) return;
  // One shared cursor across `limit` workers, rather than pre-slicing into
  // chunks: a chunked version runs at the pace of each chunk's slowest member
  // and idles the rest, which on this fan-out (a few big coding-agent threads
  // among many small ones) is most of the pool most of the time.
  let next = 0;
  const worker = async (): Promise<void> => {
    for (;;) {
      const index = next++;
      if (index >= items.length) return;
      try {
        await run(items[index]);
      } catch (err) {
        // Swallowed by contract, see above: the item's own function owns its
        // error surface, and this loop only owns keeping the pool moving. Warn
        // anyway, so a future caller that passes a non-total function gets a
        // signal instead of silence. Both task functions passed today
        // (`refreshThreadEvents`, `loadThreadEvents`) catch internally, so this
        // never fires.
        console.warn('[concurrentPool] task threw; the pool swallowed it and continued', err);
      }
    }
  };
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, worker));
}
