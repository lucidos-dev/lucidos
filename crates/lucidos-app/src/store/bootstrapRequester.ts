import { ApiError } from '../api/client/_core';
import {
  fetchBootstrap,
  type BootstrapRead, type BootstrapSliceBody, type BootstrapSliceKey, type BootstrapSnapshot, type SliceResult,
} from '../api/client/bootstrap';
import {
  beginSliceRead, failUnsentSliceRead, joinSliceLoadsDuring, releaseSliceRead, settleSliceRead, sliceLoadStarts, type BootstrapSlice, type SliceLanding,
} from './bootstrapSliceLoads';

export interface BootstrapSliceEntry<Body> {
  slice: BootstrapSlice<Body, unknown>;
  load: () => Promise<void>;
  after?: BootstrapSliceKey;
}

export type BootstrapSliceTable = { [Key in BootstrapSliceKey]: BootstrapSliceEntry<BootstrapSliceBody<Key>> };

export function sliceEntry<Body, Ctx>(
  slice: BootstrapSlice<Body, Ctx>,
  load: () => Promise<void>,
  after?: BootstrapSliceKey,
): BootstrapSliceEntry<Body> {
  return { slice: slice as BootstrapSlice<Body, unknown>, load, after };
}

export interface BootstrapRequestOptions {
  gapRepair?: () => void;
}

interface StartedSlice {
  key: BootstrapSliceKey;
  entry: BootstrapSliceEntry<unknown>;
  ctx: unknown;
  startsAtRequest: number;
}

function landingOf(result: SliceResult<unknown>): SliceLanding<unknown> {
  try {
    return 'error' in result
      ? { error: new ApiError(result.error.http_code, result.error.message) }
      : { body: result.data };
  } catch (error) {
    return { error };
  }
}

export function createBootstrapRequester(
  slices: BootstrapSliceTable,
  read: (etag: string | null) => Promise<BootstrapRead> = fetchBootstrap,
) {
  let held: { snapshot: BootstrapSnapshot; etag: string | null } | null = null;
  let inFlight: Promise<void> | null = null;
  let askAgain = false;
  let heldFrames: Array<() => void> = [];

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
    await loadNarrowly(missing.filter(({ key, entry, ctx, startsAtRequest }) => {
      const newest = sliceLoadStarts(key) === startsAtRequest;
      if (!newest) releaseSliceRead(key, startsAtRequest, entry.slice, ctx);
      return newest;
    }));
  }

  async function loadNarrowly(started: StartedSlice[]): Promise<void> {
    const loads = new Map<BootstrapSliceKey, Promise<void>>();
    for (const { key, entry, ctx, startsAtRequest } of started) {
      releaseSliceRead(key, startsAtRequest, entry.slice, ctx);
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
    requestBootstrap(options: BootstrapRequestOptions = {}): Promise<void> {
      if (inFlight) askAgain = true;
      else inFlight = run();
      const bootstrap = inFlight;
      if (options.gapRepair) joinSliceLoadsDuring(bootstrap, options.gapRepair);
      return bootstrap;
    },

    holdGlobalFrame(apply: () => void): void {
      if (inFlight) heldFrames.push(apply);
      else apply();
    },
  };
}
