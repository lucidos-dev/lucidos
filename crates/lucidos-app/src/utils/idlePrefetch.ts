
type Preloadable = { preload(): Promise<unknown> };

const waiting = new Set<Preloadable>();
let started = false;

function whenIdle(run: () => void): void {
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
