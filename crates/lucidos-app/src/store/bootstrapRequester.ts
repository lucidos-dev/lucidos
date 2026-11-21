import { ApiError } from '../api/client/_core';
import {
  fetchBootstrap,
  type BootstrapRead, type BootstrapSliceBody, type BootstrapSliceKey, type BootstrapSnapshot, type SliceResult,
} from '../api/client/bootstrap';
import {
  beginSliceRead, failUnsentSliceRead, joinSliceLoadsDuring, releaseSliceRead, settleSliceRead, sliceLoadStarts, type BootstrapSlice, type SliceLanding,
} from './bootstrapSliceLoads';

/** A slice's transitions, with the narrow loader that reads it when no
 *  snapshot does: the engine predates the route, or the slice. */
export interface BootstrapSliceEntry<Body> {
  slice: BootstrapSlice<Body, unknown>;
  load: () => Promise<void>;
  /** When this bootstrap reads both narrowly, `load` runs once this slice's
   *  loader has settled. The named slice must sit earlier in the table. */
  after?: BootstrapSliceKey;
}

/** Every slice key, so a slice added to the snapshot without a row fails `tsc`. */
export type BootstrapSliceTable = { [Key in BootstrapSliceKey]: BootstrapSliceEntry<BootstrapSliceBody<Key>> };

/** A table row. The requester only hands a slice's context back to it, so the
 *  context type stops here. */
export function sliceEntry<Body, Ctx>(
  slice: BootstrapSlice<Body, Ctx>,
  load: () => Promise<void>,
  after?: BootstrapSliceKey,
): BootstrapSliceEntry<Body> {
  return { slice: slice as BootstrapSlice<Body, unknown>, load, after };
}

export interface BootstrapRequestOptions {
  /** Runs at once. A slice loader it reaches waits for this bootstrap instead
   *  of reading its own route. */
  gapRepair?: () => void;
}

interface StartedSlice {
  key: BootstrapSliceKey;
  entry: BootstrapSliceEntry<unknown>;
  ctx: unknown;
  startsAtRequest: number;
}

/** One slice's result as a landing. A result that cannot be read fails its
 *  own slice, never the slices after it. */
function landingOf(result: SliceResult<unknown>): SliceLanding<unknown> {
  try {
    return 'error' in result
      ? { error: new ApiError(result.error.http_code, result.error.message) }
      : { body: result.data };
  } catch (error) {
    return { error };
  }
}

/** The ordering rules of the bootstrap snapshot, over any slice table (see
 *  `docs/plans/2026-11-19-bootstrap-snapshot.md` § Ordering rules). */
export function createBootstrapRequester(
  slices: BootstrapSliceTable,
  read: (etag: string | null) => Promise<BootstrapRead> = fetchBootstrap,
) {
  // Memory only: nothing here outlives the document.
  let held: { snapshot: BootstrapSnapshot; etag: string | null } | null = null;
  let inFlight: Promise<void> | null = null;
  let askAgain = false;
  let heldFrames: Array<() => void> = [];

  /** The bootstrap counts as a start of every slice, so a narrow read that
   *  left before it cannot overwrite the snapshot. Every slice begins before
   *  any start is tracked, so a `begin` that throws strands no tracked start.
   *  Every slice already begun settles, so none stays loading. */
  function startSlices(): StartedSlice[] {
    const begun: Omit<StartedSlice, 'startsAtRequest'>[] = [];
    try {
      for (const key of Object.keys(slices) as BootstrapSliceKey[]) {
        const entry = slices[key] as BootstrapSliceEntry<unknown>;
        begun.push({ key, entry, ctx: entry.slice.begin() });
      }
    } catch (error) {
      for (const { key, entry, ctx } of begun) failUnsentSliceRead(key, entry.slice, ctx, error);
      throw error;
    }
    return begun.map((slice) => ({ ...slice, startsAtRequest: beginSliceRead(slice.key) }));
  }

  /** Every settle starts in one synchronous run, in table order. So no narrow
   *  read starts between a slice's ordering check and its apply, and a slice
   *  whose apply awaits delays no other slice. */
  async function applySnapshot(snapshot: BootstrapSnapshot, started: StartedSlice[]): Promise<void> {
    const missing: StartedSlice[] = [];
    await Promise.all(started.map((slice) => {
      const { key, entry, ctx, startsAtRequest } = slice;
      const result = snapshot[key] as SliceResult<unknown> | undefined;
      if (!result) {
        missing.push(slice);
        return;
      }
      return settleSliceRead(key, startsAtRequest, entry.slice, ctx, landingOf(result));
    }));
    // An engine that serves the route can still predate a slice, whose own
    // route answers. Its loader runs after the applies, as `after` requires.
    // A narrow read that started during them settles the slice instead.
    await loadNarrowly(missing.filter(({ key, entry, ctx, startsAtRequest }) => {
      const newest = sliceLoadStarts(key) === startsAtRequest;
      if (!newest) releaseSliceRead(key, startsAtRequest, entry.slice, ctx);
      return newest;
    }));
  }

  /** Each slice loader reads its own route. Every loader settles before this
   *  does, so no read outlives the bootstrap. The first rejection is rethrown. */
  async function loadNarrowly(started: StartedSlice[]): Promise<void> {
    const loads = new Map<BootstrapSliceKey, Promise<void>>();
    for (const { key, entry, ctx, startsAtRequest } of started) {
      releaseSliceRead(key, startsAtRequest, entry.slice, ctx);
      // Async, so a loader that throws before its promise still settles below.
      const load = async () => entry.load();
      const earlier = entry.after === undefined ? undefined : loads.get(entry.after);
      loads.set(key, earlier ? earlier.then(load, load) : load());
    }
    const rejected = (await Promise.allSettled(loads.values())).find((load) => load.status === 'rejected');
    if (rejected) throw rejected.reason;
  }

  async function failAll(started: StartedSlice[], error: unknown): Promise<void> {
    await Promise.all(started.map(({ key, entry, ctx, startsAtRequest }) =>
      settleSliceRead(key, startsAtRequest, entry.slice, ctx, { error })));
  }

  async function pass(): Promise<void> {
    const started = startSlices();
    let answer: BootstrapRead;
    try {
      answer = await read(held?.etag ?? null);
    } catch (error) {
      return failAll(started, error);
    }
    // An engine older than the route (docs/temporary-measures.md § Bootstrap 404 fallback).
    if (answer.kind === 'route-missing') return loadNarrowly(started);
    if (answer.kind === 'snapshot') held = { snapshot: answer.snapshot, etag: answer.etag };
    if (held) await applySnapshot(held.snapshot, started);
    else await failAll(started, new Error('The bootstrap answered "not modified" before any snapshot'));
  }

  /** A frame that throws is reported as uncaught, as it is outside a
   *  bootstrap, and costs no later frame its turn. */
  function releaseHeldFrames(): void {
    const frames = heldFrames;
    heldFrames = [];
    for (const apply of frames) {
      try {
        apply();
      } catch (error) {
        queueMicrotask(() => { throw error; });
      }
    }
  }

  async function run(): Promise<void> {
    try {
      // A pass that rejects still owes the repeat a later caller asked for.
      // Only the last pass decides how the bootstrap ends.
      let rejection: { error: unknown } | null;
      do {
        askAgain = false;
        rejection = null;
        try {
          await pass();
        } catch (error) {
          rejection = { error };
        } finally {
          releaseHeldFrames();
        }
      } while (askAgain);
      if (rejection) throw rejection.error;
    } finally {
      inFlight = null;
    }
  }

  return {
    /** Read the snapshot and apply it. A call during a running bootstrap never
     *  joins that read, which may predate the caller's cause: it repeats after. */
    requestBootstrap(options: BootstrapRequestOptions = {}): Promise<void> {
      if (inFlight) askAgain = true;
      else inFlight = run();
      const bootstrap = inFlight;
      if (options.gapRepair) joinSliceLoadsDuring(bootstrap, options.gapRepair);
      return bootstrap;
    },

    /** Apply a global stream frame now, or after the snapshot that is out. */
    holdGlobalFrame(apply: () => void): void {
      if (inFlight) heldFrames.push(apply);
      else apply();
    },
  };
}
