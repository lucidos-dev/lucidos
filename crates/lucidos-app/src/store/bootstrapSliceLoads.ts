import type { BootstrapSliceBody, BootstrapSliceKey } from '../api/client/bootstrap';

/**
 * What a slice loader and the bootstrap snapshot share (see `docs/glossary.md`
 * § Bootstrap snapshot). Imports nothing from the store, so every action module
 * can use it.
 */

/** One slice's state transitions, split from the request that feeds them.
 *  `begin` runs when a read leaves and returns what the others need. Every
 *  context it returns ends in exactly one of them. */
export interface BootstrapSlice<Body, Ctx = void> {
  begin(): Ctx;
  apply(body: Body, ctx: Ctx): void | Promise<void>;
  fail(error: unknown, ctx: Ctx): void;
  /** The read `begin` announced settles nothing: a newer read already applied,
   *  a newer read is still out, or another loader reads the slice instead.
   *  For a slice whose context holds something open until it settles. */
  abandon?(ctx: Ctx): void;
}

/** The slice for one snapshot key, so its body type has one definition. */
export type SliceOf<Key extends BootstrapSliceKey, Ctx = void> = BootstrapSlice<BootstrapSliceBody<Key>, Ctx>;

const starts = new Map<BootstrapSliceKey, number>();
const applied = new Map<BootstrapSliceKey, number>();
/** The reads that will still settle the slice. Each leaves it as it settles
 *  or releases, so a set never holds more than the reads in flight. */
const outstanding = new Map<BootstrapSliceKey, Set<number>>();
let joinable: Promise<void> | null = null;

/** How many reads of this slice have started, narrow or bootstrap. A read's
 *  count orders it against every other read of the slice. */
export function sliceLoadStarts(key: BootstrapSliceKey): number {
  return starts.get(key) ?? 0;
}

/** What a read of a slice brought back. */
export type SliceLanding<Body> = { body: Body } | { error: unknown };

/** Count a read that will settle through `settleSliceRead`, and return its
 *  count. It must end in exactly one settle or `releaseSliceRead`. */
export function beginSliceRead(key: BootstrapSliceKey): number {
  const start = countSliceStart(key);
  trackSliceRead(key, start);
  return start;
}

/** `startSliceLoad` for a loader that settles its own read. A promise means
 *  the bootstrap is reading this slice. A count is a read begun as
 *  `beginSliceRead` begins one, with the same obligation to settle. */
export function beginSliceLoad(key: BootstrapSliceKey): Promise<void> | number {
  return joinable ?? beginSliceRead(key);
}

function trackSliceRead(key: BootstrapSliceKey, start: number): void {
  const reads = outstanding.get(key) ?? new Set<number>();
  reads.add(start);
  outstanding.set(key, reads);
}

/** A counted read that will settle nothing: another loader reads the slice. */
export function releaseSliceRead<Ctx>(key: BootstrapSliceKey, start: number, slice: BootstrapSlice<unknown, Ctx>, ctx: Ctx): void {
  outstanding.get(key)?.delete(start);
  slice.abandon?.(ctx);
}

/** Settle a context whose read never left. Such a read is the newest, so it
 *  fails the slice unless another read is still out to settle it. */
export function failUnsentSliceRead<Ctx>(key: BootstrapSliceKey, slice: BootstrapSlice<unknown, Ctx>, ctx: Ctx, error: unknown): void {
  if (outstanding.get(key)?.size) slice.abandon?.(ctx);
  else slice.fail(error, ctx);
}

function newerReadSettles(key: BootstrapSliceKey, start: number): boolean {
  if ((applied.get(key) ?? 0) > start) return true;
  for (const out of outstanding.get(key) ?? []) if (out > start) return true;
  return false;
}

/** Settle one read under the slice's ordering rule (`docs/glossary.md`
 *  § Bootstrap snapshot). An answer applies unless a newer read already
 *  applied. A failure fails the slice unless a newer read applied or is still
 *  out. Every other read abandons its context. */
export async function settleSliceRead<Key extends BootstrapSliceKey, Body, Ctx>(
  key: Key,
  start: number,
  slice: BootstrapSlice<Body, Ctx>,
  ctx: Ctx,
  landed: SliceLanding<Body>,
): Promise<void> {
  outstanding.get(key)?.delete(start);
  if ('error' in landed) {
    if (newerReadSettles(key, start)) slice.abandon?.(ctx);
    else slice.fail(landed.error, ctx);
    return;
  }
  if (start <= (applied.get(key) ?? 0)) {
    slice.abandon?.(ctx);
    return;
  }
  const appliedBefore = applied.get(key) ?? 0;
  applied.set(key, start);
  try {
    await slice.apply(landed.body, ctx);
  } catch (error) {
    // An answer the slice could not apply never applied, so an older good
    // answer that lands later may still apply. It fails like any failed read.
    if (applied.get(key) === start) applied.set(key, appliedBefore);
    if (newerReadSettles(key, start)) slice.abandon?.(ctx);
    else slice.fail(error, ctx);
  }
}

/** Count a read of this slice as started, and return its count. */
function countSliceStart(key: BootstrapSliceKey): number {
  const count = sliceLoadStarts(key) + 1;
  starts.set(key, count);
  return count;
}

/** The first line of every slice loader. A promise means the bootstrap is
 *  already reading this slice: return it and fetch nothing. */
export function startSliceLoad(key: BootstrapSliceKey): Promise<void> | null {
  if (joinable) return joinable;
  countSliceStart(key);
  return null;
}

/** Run `section` with every slice loader it reaches answered by `bootstrap`. */
export function joinSliceLoadsDuring(bootstrap: Promise<void>, section: () => void): void {
  joinable = bootstrap;
  try {
    section();
  } finally {
    joinable = null;
  }
}

/** A slice loader with no guard of its own: begin, read, then apply or fail.
 *  `read` must name the client inside its body, so a throwing lookup fails the slice. */
export function loadSlice<Key extends BootstrapSliceKey, Ctx>(
  key: Key,
  slice: SliceOf<Key, Ctx>,
  read: () => Promise<BootstrapSliceBody<Key>>,
): Promise<void> {
  return startSliceLoad(key) ?? readSlice(key, slice, read);
}

/** A slice loader whose concurrent callers share one read. The read leaves
 *  the shared slot when its request lands, before it settles. So a later
 *  caller sends its own read rather than joining a settled one. */
export function sharedSliceLoader<Key extends BootstrapSliceKey, Ctx>(
  key: Key,
  slice: SliceOf<Key, Ctx>,
  read: () => Promise<BootstrapSliceBody<Key>>,
): { load(): Promise<void>; inFlight(): Promise<void> | null } {
  let inFlight: Promise<void> | null = null;
  return {
    load() {
      if (inFlight) return inFlight;
      const joined = startSliceLoad(key);
      if (joined) return joined;
      let landed = false;
      let load: Promise<void> | undefined;
      load = readSlice(key, slice, async () => {
        try {
          return await read();
        } finally {
          landed = true;
          if (inFlight === load) inFlight = null;
        }
      });
      // A read that threw before its promise has already landed.
      if (landed) return load;
      inFlight = load;
      // A `begin` that throws rejects before the read leaves, so nothing lands.
      load.catch(() => { if (inFlight === load) inFlight = null; });
      return load;
    },
    inFlight: () => inFlight,
  };
}

/** The read behind a slice loader that has already called `startSliceLoad`.
 *  A loader that shares its in-flight read shares this one, never the join. */
export async function readSlice<Key extends BootstrapSliceKey, Ctx>(
  key: Key,
  slice: SliceOf<Key, Ctx>,
  read: () => Promise<BootstrapSliceBody<Key>>,
): Promise<void> {
  const start = sliceLoadStarts(key);
  const ctx = slice.begin();
  trackSliceRead(key, start);
  let landed: SliceLanding<BootstrapSliceBody<Key>>;
  try {
    landed = { body: await read() };
  } catch (error) {
    landed = { error };
  }
  return settleSliceRead(key, start, slice, ctx, landed);
}
