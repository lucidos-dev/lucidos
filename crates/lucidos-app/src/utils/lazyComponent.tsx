import { signal } from '@preact/signals';
import { h, type ComponentType } from 'preact';
import { showToast } from '../store/store';

/**
 * Render a code-split component lazily — fetched on first mount, then cached.
 * Returns `null` until the chunk arrives. Pair with a signal-gated mount site
 * (`{open.value && <Lazy />}`) so the chunk only fetches on first open;
 * mounting unconditionally would defeat the split.
 *
 * Failed loads surface a toast and reset the loading flag, so re-mounting
 * (e.g. user reopens the overlay) retries. The common cause is a stale bundle
 * after deploy — the toast tells the user to refresh.
 */
export function lazyComponent<P>(
  loader: () => Promise<ComponentType<P> | { default: ComponentType<P> }>,
): ComponentType<P> {
  const cached = signal<ComponentType<P> | null>(null);
  let loading: Promise<void> | null = null;

  function start(): void {
    if (loading || cached.value) return;
    loading = loader().then(
      (mod) => {
        cached.value = (typeof mod === 'object' && mod !== null && 'default' in mod
          ? mod.default
          : mod) as ComponentType<P>;
      },
      (err) => {
        loading = null;
        console.error('[lazyComponent] load failed', err);
        showToast('Failed to load. Refresh the page to try again.', 'error');
      },
    );
  }

  function Lazy(props: P) {
    if (!cached.value) {
      start();
      return null;
    }
    // Use h() instead of JSX so the unconstrained P doesn't have to satisfy
    // IntrinsicAttributes — Preact resolves attribute typing at runtime.
    return h(cached.value, props as never);
  }
  Lazy.displayName = 'Lazy';
  return Lazy as ComponentType<P>;
}
