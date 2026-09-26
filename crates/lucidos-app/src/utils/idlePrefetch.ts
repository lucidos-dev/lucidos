/**
 * Idle prefetch: load the chunk of a surface that only opens on demand once
 * the boot splash has lifted (ADR 0288). The first frame never waits for it,
 * and its first open finds it in memory rather than on the network.
 *
 * `dismissBootSplash` starts it, so every path that lifts the splash does.
 * A surface registered after that is scheduled at once.
 */

type Preloadable = { preload(): Promise<unknown> };

const waiting = new Set<Preloadable>();
let started = false;

function whenIdle(run: () => void): void {
  // WebKit has no requestIdleCallback, so a macrotask stands in there.
  if (typeof requestIdleCallback === 'function') requestIdleCallback(run, { timeout: 2000 });
  else setTimeout(run, 0);
}

export function prefetchWhenIdle(surface: Preloadable): void {
  if (started) whenIdle(() => void surface.preload());
  else waiting.add(surface);
}

export function startIdlePrefetch(): void {
  if (started) return;
  started = true;
  for (const surface of waiting) whenIdle(() => void surface.preload());
  waiting.clear();
}
