import { beforeEach, expect, it, vi } from 'vitest';
const m = vi.hoisted(() => ({ idle: undefined as undefined | { preload(): Promise<unknown> }, preload: vi.fn(), start: vi.fn(), toast: vi.fn() }));
vi.mock('../utils/idlePrefetch', () => ({ prefetchWhenIdle: (surface: { preload(): Promise<unknown> }) => { m.idle = surface; } }));
vi.mock('../utils/lazyComponent', () => ({ lazyModule: () => ({ preload: m.preload, get: () => ({ startBackgroundChecks: m.start }) }) }));
vi.mock('./store', () => ({ showToast: m.toast, removeToast: vi.fn() }));
import { startDeferredBackgroundChecks } from './deferredBackgroundChecks';

const checks = { refreshEngine: vi.fn(), refreshRelease: vi.fn(), refreshSlowness: vi.fn(), stop: vi.fn() };
beforeEach(() => {
  vi.clearAllMocks();
  m.preload.mockReset().mockResolvedValue(true);
  m.start.mockReturnValue(checks);
});

it('waits for idle and starts initial snapshots once, then forwards wake reads', async () => {
  const background = startDeferredBackgroundChecks();
  background.refreshEngine();
  background.refreshRelease();
  expect(m.preload).not.toHaveBeenCalled();
  expect(m.start).not.toHaveBeenCalled();
  await Promise.all([m.idle!.preload(), m.idle!.preload()]);
  expect(m.start).toHaveBeenCalledOnce();
  background.refreshEngine();
  background.refreshRelease();
  background.refreshSlowness();
  expect(checks.refreshEngine).toHaveBeenCalledOnce();
  expect(checks.refreshRelease).toHaveBeenCalledOnce();
  expect(checks.refreshSlowness).toHaveBeenCalledOnce();
  background.stop();
  expect(checks.stop).toHaveBeenCalledOnce();
});

it('offers Retry after a failed chunk and then starts the checks', async () => {
  m.preload.mockResolvedValueOnce(false);
  const background = startDeferredBackgroundChecks();
  await m.idle!.preload();
  expect(m.start).not.toHaveBeenCalled();
  m.toast.mock.calls[0][2].action.onClick();
  await vi.waitFor(() => expect(m.start).toHaveBeenCalledOnce());
  background.stop();
});

it('prevents late installation when stopped during the fetch', async () => {
  let complete!: (loaded: boolean) => void;
  m.preload.mockImplementationOnce(() => new Promise<boolean>((resolve) => { complete = resolve; }));
  const background = startDeferredBackgroundChecks();
  const loading = m.idle!.preload();
  background.stop();
  complete(true);
  await loading;
  expect(m.start).not.toHaveBeenCalled();
  expect(m.toast).not.toHaveBeenCalled();
});
