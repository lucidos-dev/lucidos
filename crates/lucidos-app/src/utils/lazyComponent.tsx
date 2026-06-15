import { signal } from '@preact/signals';
import { h, type ComponentType } from 'preact';
import { showToast } from '../store/store';
import { reloadForStaleChunk } from '../hooks/sw-update';

/**
 * Render a code-split component lazily — fetched on first mount, then cached.
 * Returns `null` until the chunk arrives. Pair with a signal-gated mount site
 * (`{open.value && <Lazy />}`) so the chunk only fetches on first open;
 * mounting unconditionally would defeat the split.
 *
 * The common failure is a stale bundle: a rebuild shipped while this tab kept
 * the old build in memory, so the lazy chunk's old hashed URL now 404s. We
 * auto-reload to the new build (`reloadForStaleChunk`) instead of stranding the
 * user — and only fall back to a "Refresh the page" toast if the loop guard
 * trips. The `loading` flag is reset either way, so a re-mount retries.
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
        // Almost always a stale chunk after a rebuild — reload to the new build.
        // Toast only if the loop guard already reloaded us recently.
        if (!reloadForStaleChunk()) {
          showToast('Failed to load. Refresh the page to try again.', 'error');
        }
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
