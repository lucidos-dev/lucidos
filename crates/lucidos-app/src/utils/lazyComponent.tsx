import { signal } from '@preact/signals';
import { h, type ComponentType } from 'preact';
import { showToast } from '../store/store';
import { reloadForStaleChunk } from '../hooks/sw-update';

/** A code-split component, plus a way to load its chunk before anything mounts it. */
export type LazyComponent<P> = ComponentType<P> & {
  /** Start the load a first mount would start, and share it. Resolves `true`
   *  once the component is ready, or `false` once a failure has been handled.
   *  Never rejects. */
  preload(): Promise<boolean>;
};

/**
 * Render a code-split component lazily, fetched on first mount or `preload()`,
 * then cached. Returns `null` until the chunk arrives. Pair with a signal-gated
 * mount site (`{open.value && <Lazy />}`) so the chunk only fetches on first
 * open; mounting unconditionally would defeat the split.
 *
 * The common failure is a stale bundle: a rebuild shipped while this tab kept
 * the old build in memory, so the lazy chunk's old hashed URL now 404s. We
 * auto-reload to the new build (`reloadForStaleChunk`) instead of stranding the
 * user, and only fall back to a "Refresh the page" toast if the loop guard
 * trips. The `loading` flag is reset either way, so a re-mount retries.
 */
export function lazyComponent<P>(
  loader: () => Promise<ComponentType<P> | { default: ComponentType<P> }>,
): LazyComponent<P> {
  const cached = signal<ComponentType<P> | null>(null);
  let loading: Promise<boolean> | null = null;

  function start(): Promise<boolean> {
    if (cached.value) return Promise.resolve(true);
    if (loading) return loading;
    loading = loader().then(
      (mod) => {
        cached.value = (typeof mod === 'object' && mod !== null && 'default' in mod
          ? mod.default
          : mod) as ComponentType<P>;
        return true;
      },
      (err) => {
        loading = null;
        console.error('[lazyComponent] load failed', err);
        // Almost always a stale chunk after a rebuild, so reload to the new build.
        // Toast only if the loop guard already reloaded us recently.
        if (!reloadForStaleChunk()) {
          showToast('Failed to load. Refresh the page to try again.', 'error');
        }
        return false;
      },
    );
    return loading;
  }

  function Lazy(props: P) {
    if (!cached.value) {
      void start();
      return null;
    }
    // Use h() instead of JSX so the unconstrained P doesn't have to satisfy
    // IntrinsicAttributes. Preact resolves attribute typing at runtime.
    return h(cached.value, props as never);
  }
  Lazy.displayName = 'Lazy';
  Lazy.preload = start;
  return Lazy as LazyComponent<P>;
}

/** An open waiting on a chunk. `pending` goes false once it opens or is
 *  cancelled, whoever cancelled it. */
export interface PendingOpen {
  readonly pending: boolean;
  cancel(): void;
}

/** Run `open` once `lazy`'s chunk is in memory, so a surface never opens empty,
 *  and never after a failed load.
 *  Warm, that is the next microtask. A second press on the trigger calls
 *  `cancel` to take the open back. A press anywhere else cancels it too: the
 *  user has moved on, and a late open would land on whatever is next. */
export function whenLoaded(
  lazy: { preload(): Promise<boolean> },
  open: () => void,
  trigger: Element,
  doc: Document = document,
): PendingOpen {
  const onPress = (e: PointerEvent) => {
    if (!trigger.contains(e.target as Node)) handle.cancel();
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
