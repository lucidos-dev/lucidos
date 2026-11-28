import type { BootstrapSliceBody, BootstrapSliceKey } from '../api/client/bootstrap';


export interface BootstrapSlice<Body, Ctx = void> {
  begin(): Ctx;
  apply(body: Body, ctx: Ctx): void | Promise<void>;
  fail(error: unknown, ctx: Ctx): void;
  abandon?(ctx: Ctx): void;
}

export type SliceOf<Key extends BootstrapSliceKey, Ctx = void> = BootstrapSlice<BootstrapSliceBody<Key>, Ctx>;

const starts = new Map<BootstrapSliceKey, number>();
const applied = new Map<BootstrapSliceKey, number>();
const outstanding = new Map<BootstrapSliceKey, Set<number>>();
let joinable: Promise<void> | null = null;

export function sliceLoadStarts(key: BootstrapSliceKey): number {
  return starts.get(key) ?? 0;
}

export type SliceLanding<Body> = { body: Body } | { error: unknown };

export function beginSliceRead(key: BootstrapSliceKey): number {
  const start = countSliceStart(key);
  trackSliceRead(key, start);
  return start;
}

export function beginSliceLoad(key: BootstrapSliceKey): Promise<void> | number {
  return joinable ?? beginSliceRead(key);
}

function trackSliceRead(key: BootstrapSliceKey, start: number): void {
  const reads = outstanding.get(key) ?? new Set<number>();
  reads.add(start);
  outstanding.set(key, reads);
}

export function releaseSliceRead<Ctx>(key: BootstrapSliceKey, start: number, slice: BootstrapSlice<unknown, Ctx>, ctx: Ctx): void {
  outstanding.get(key)?.delete(start);
  slice.abandon?.(ctx);
}

export function failUnsentSliceRead<Ctx>(key: BootstrapSliceKey, slice: BootstrapSlice<unknown, Ctx>, ctx: Ctx, error: unknown): void {
  if (outstanding.get(key)?.size) slice.abandon?.(ctx);
  else slice.fail(error, ctx);
}

function newerReadSettles(key: BootstrapSliceKey, start: number): boolean {
  if ((applied.get(key) ?? 0) > start) return true;
  for (const out of outstanding.get(key) ?? []) if (out > start) return true;
  return false;
}

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
    if (applied.get(key) === start) applied.set(key, appliedBefore);
    if (newerReadSettles(key, start)) slice.abandon?.(ctx);
    else slice.fail(error, ctx);
  }
}

function countSliceStart(key: BootstrapSliceKey): number {
  const count = sliceLoadStarts(key) + 1;
  starts.set(key, count);
  return count;
}

export function startSliceLoad(key: BootstrapSliceKey): Promise<void> | null {
  if (joinable) return joinable;
  countSliceStart(key);
  return null;
}

export function joinSliceLoadsDuring(bootstrap: Promise<void>, section: () => void): void {
  joinable = bootstrap;
  try {
    section();
  } finally {
    joinable = null;
  }
}

export function loadSlice<Key extends BootstrapSliceKey, Ctx>(
  key: Key,
  slice: SliceOf<Key, Ctx>,
  read: () => Promise<BootstrapSliceBody<Key>>,
): Promise<void> {
  return startSliceLoad(key) ?? readSlice(key, slice, read);
}

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
      if (landed) return load;
      inFlight = load;
      load.catch(() => { if (inFlight === load) inFlight = null; });
      return load;
    },
    inFlight: () => inFlight,
  };
}

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
