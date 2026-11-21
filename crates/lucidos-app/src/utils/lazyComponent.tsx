import { signal } from '@preact/signals';
import { h, type ComponentType } from 'preact';
import { showToast } from '../store/store';
import { reloadForStaleChunk } from '../hooks/sw-update';
import { postClientLog } from './clientLog';
import { CLIENT_BUILD_ID } from 'virtual:build-id';

/** A code-split component, plus a way to load its chunk before anything mounts it. */
export type LazyComponent<P> = ComponentType<P> & {
  /** Start the load a first mount would start, and share it. Resolves `true`
   *  once the component is ready, or `false` once a failure has been handled.
   *  Never rejects. */
  preload(): Promise<boolean>;
  /** Whether the component is in memory, so a mount renders it at once. */
  isLoaded(): boolean;
};

/** The chunk a built loader imports, read from its own source, which
 *  Vite emits as `() => preload(() => import("./Name-hash.js"), …)`. WebKit's
 *  import error names no URL, so this says which import failed. The file at
 *  fault may be one of its static imports, which the service worker logs. */
export function importedChunkOf(load: () => unknown): string | null {
  return /import\(\s*["'`]([^"'`]+)["'`]\s*\)/.exec(String(load))?.[1] ?? null;
}

/** Fetch a failed imported chunk again, the way its import did, and log what came
 *  back. That tells a missing file (a stale build) from a body the browser
 *  refused. It goes through the service worker and the HTTP cache alike. */
export async function probeFailedChunk(file: string): Promise<void> {
  const controlled = typeof navigator !== 'undefined' && !!navigator.serviceWorker?.controller;
  const base = { importedChunk: file, build: CLIENT_BUILD_ID, controlled };
  try {
    // A Vite chunk specifier is relative to the assets directory it sits in.
    const response = await fetch(new URL(file, new URL('assets/', document.baseURI)));
    const bytes = (await response.arrayBuffer()).byteLength;
    postClientLog('chunk', 'probe', {
      ...base,
      status: response.status,
      type: response.type,
      contentType: response.headers.get('content-type'),
      bytes,
    });
  } catch (err) {
    postClientLog('chunk', 'probe', { ...base, error: String(err).slice(0, 300) });
  }
}

/**
 * Load a chunk once and keep what it holds. Resolves `true` once kept, or
 * `false` once a failure has been handled. Never rejects.
 *
 * The common failure is a stale bundle: a rebuild shipped while this tab kept
 * the old build in memory, so the chunk's old hashed URL now 404s. We
 * auto-reload to the new build (`reloadForStaleChunk`) instead of stranding the
 * user, and only fall back to a "Refresh the page" toast if the loop guard
 * trips. The in-flight load is dropped either way, so the next ask retries.
 */
function chunkLoader<T>(load: () => Promise<T>, keep: (value: T) => void, kept: () => boolean): () => Promise<boolean> {
  let loading: Promise<boolean> | null = null;
  return () => {
    if (kept()) return Promise.resolve(true);
    if (loading) return loading;
    loading = load().then(
      (value) => {
        keep(value);
        return true;
      },
      (err) => {
        loading = null;
        console.error('[lazyComponent] load failed', err);
        const importedChunk = importedChunkOf(load);
        postClientLog('chunk', 'load_failed', { message: String(err).slice(0, 300), importedChunk });
        // Almost always a stale chunk after a rebuild, so reload to the new build.
        // Toast only if the loop guard already reloaded us recently. Probe only
        // then: a reload would cut the probe short, and a healed load needs none.
        if (!reloadForStaleChunk()) {
          showToast('Failed to load. Refresh the page to try again.', 'error');
          if (importedChunk) void probeFailedChunk(importedChunk);
        }
        return false;
      },
    );
    return loading;
  };
}

/**
 * Render a code-split component lazily, fetched on first mount or `preload()`,
 * then cached. Renders `placeholder`, or nothing, until the chunk arrives.
 * Pair a component with no placeholder with a signal-gated mount site
 * (`{open.value && <Lazy />}`), so the chunk only fetches on first open.
 */
export function lazyComponent<P>(
  loader: () => Promise<ComponentType<P> | { default: ComponentType<P> }>,
  placeholder?: ComponentType<P>,
): LazyComponent<P> {
  const cached = signal<ComponentType<P> | null>(null);
  const start = chunkLoader(
    loader,
    (mod) => {
      cached.value = (typeof mod === 'object' && mod !== null && 'default' in mod
        ? mod.default
        : mod) as ComponentType<P>;
    },
    () => cached.peek() !== null,
  );

  function Lazy(props: P) {
    if (!cached.value) {
      void start();
      return placeholder ? h(placeholder, props as never) : null;
    }
    // Use h() instead of JSX so the unconstrained P doesn't have to satisfy
    // IntrinsicAttributes. Preact resolves attribute typing at runtime.
    return h(cached.value, props as never);
  }
  Lazy.displayName = 'Lazy';
  Lazy.preload = start;
  Lazy.isLoaded = () => cached.peek() !== null;
  return Lazy as LazyComponent<P>;
}

/** A code-split module of code rather than a component. */
export type LazyModule<M> = Pick<LazyComponent<never>, 'preload' | 'isLoaded'> & {
  /** The loaded module. Read it only once `isLoaded()`: an open gated on
   *  `whenLoaded` is the usual way to know. */
  get(): M;
};

export function lazyModule<M>(loader: () => Promise<M>): LazyModule<M> {
  let loaded: M | null = null;
  return {
    preload: chunkLoader(loader, (mod) => { loaded = mod; }, () => loaded !== null),
    isLoaded: () => loaded !== null,
    get() {
      if (loaded === null) throw new Error('A lazy module was read before it loaded');
      return loaded;
    },
  };
}

/** A chunk an open can wait on: any `LazyComponent`, or a group of them. */
export type ChunkGate = Pick<LazyComponent<never>, 'preload' | 'isLoaded'>;

/** Several lazy components that share one chunk, as one gate. The first loads
 *  alone, and the rest only once it has, so a failed fetch reports once rather
 *  than once per component. */
export function chunkGroup(first: ChunkGate, ...rest: ChunkGate[]): ChunkGate {
  return {
    preload: async () => (await first.preload())
      && (await Promise.all(rest.map((part) => part.preload()))).every(Boolean),
    isLoaded: () => first.isLoaded() && rest.every((part) => part.isLoaded()),
  };
}

/** An open waiting on a chunk. `pending` goes false once it opens or is
 *  cancelled, whoever cancelled it. */
export interface PendingOpen {
  readonly pending: boolean;
  cancel(): void;
}

/** Run `open` once `lazy`'s chunk is in memory, so a surface never opens empty,
 *  and never after a failed load.
 *  Warm, that is at once, inside the press that asked. A second press on the
 *  trigger calls `cancel` to take the open back. A press anywhere else cancels
 *  it too: the user has moved on, and a late open would land on whatever is
 *  next. An open with no trigger, such as a shortcut's, any press cancels. */
export function whenLoaded(
  lazy: ChunkGate,
  open: () => void,
  trigger: Element | null,
  doc: Document = document,
): PendingOpen {
  if (lazy.isLoaded()) {
    open();
    return { pending: false, cancel() {} };
  }
  const onPress = (e: PointerEvent) => {
    if (!trigger?.contains(e.target as Node)) handle.cancel();
  };
  const handle = {
    pending: true,
    cancel() {
      handle.pending = false;
      doc.removeEventListener('pointerdown', onPress, true);
    },
  };
  doc.addEventListener('pointerdown', onPress, true);
  void lazy.preload().then((loaded) => {
    if (!handle.pending) return;
    handle.cancel();
    // A failed load has already reloaded the page or said so in a toast.
    if (loaded) open();
  });
  return handle;
}
