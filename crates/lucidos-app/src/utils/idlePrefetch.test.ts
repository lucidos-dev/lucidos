import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

/**
 * Idle prefetch (ADR 0288). A surface that only opens on demand loads after
 * the boot splash lifts. So the first frame never waits for it, and its first
 * open never waits for the network.
 */
describe('idle prefetch', () => {
  beforeEach(() => {
    vi.resetModules();
    vi.useFakeTimers();
  });
  afterEach(() => vi.useRealTimers());

  const lazy = () => ({ preload: vi.fn(() => Promise.resolve()) });

  it('waits for the splash to lift before loading anything', async () => {
    const { prefetchWhenIdle, startIdlePrefetch } = await import('./idlePrefetch');
    const a = lazy();
    prefetchWhenIdle(a);
    vi.runAllTimers();
    expect(a.preload).not.toHaveBeenCalled();
    startIdlePrefetch();
    vi.runAllTimers();
    expect(a.preload).toHaveBeenCalledTimes(1);
  });

  it('loads a surface registered after the splash lifted, without a second start', async () => {
    const { prefetchWhenIdle, startIdlePrefetch } = await import('./idlePrefetch');
    startIdlePrefetch();
    const late = lazy();
    prefetchWhenIdle(late);
    vi.runAllTimers();
    expect(late.preload).toHaveBeenCalledTimes(1);
  });

  it('starts once, however often the splash is dismissed', async () => {
    const { prefetchWhenIdle, startIdlePrefetch } = await import('./idlePrefetch');
    const a = lazy();
    prefetchWhenIdle(a);
    startIdlePrefetch();
    startIdlePrefetch();
    vi.runAllTimers();
    expect(a.preload).toHaveBeenCalledTimes(1);
  });

  it('is started by the splash dismissal itself, on every path that lifts it', async () => {
    // @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
    const { readFileSync } = await import('node:fs');
    const src: string = readFileSync(new URL('./bootSplash.ts', import.meta.url), 'utf8');
    const body = src.slice(src.indexOf('export function dismissBootSplash'));
    expect(body.slice(0, body.indexOf('\n}\n'))).toContain('startIdlePrefetch()');
  });
});
