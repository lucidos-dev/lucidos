import { beforeEach, expect, it, vi } from 'vitest';
import type { AppUpdateProgress } from '../../utils/tauri';
const m = vi.hoisted(() => ({ listen: vi.fn() }));
vi.mock('../../utils/tauri', () => ({ listen: m.listen, APP_UPDATE_PROGRESS_EVENT: 'app-update-progress' }));
vi.mock('../../utils/platform', () => ({ isTauri: () => true }));
beforeEach(() => { vi.resetModules(); m.listen.mockReset().mockResolvedValue(vi.fn()); });

it('keeps early progress and terminal frames until the presenter is available', async () => {
  const events = await import('./appUpdateProgressEvents');
  events.startAppUpdateProgress();
  const receive = m.listen.mock.calls[0][1];
  const downloading = { version: '1.0.0', phase: 'downloading', downloaded: 1, total: null } as AppUpdateProgress;
  const failed = { version: '1.0.0', phase: 'failed', message: 'Download interrupted' } as AppUpdateProgress;
  receive({ payload: downloading });
  receive({ payload: failed });
  const present = vi.fn();
  events.setAppUpdateProgressHandler(present);
  expect(present.mock.calls.map(([frame]) => frame)).toEqual([downloading, failed]);
  events.setAppUpdateProgressHandler(present);
  expect(present).toHaveBeenCalledTimes(2);
  events.stopAppUpdateProgress();
});

it('retries the subscription on the next start after a failed listen', async () => {
  m.listen.mockRejectedValueOnce(new Error('denied'));
  const events = await import('./appUpdateProgressEvents');
  events.startAppUpdateProgress();
  await new Promise((resolve) => setTimeout(resolve, 0));
  events.startAppUpdateProgress();
  expect(m.listen).toHaveBeenCalledTimes(2);
  events.stopAppUpdateProgress();
});

it('clears queued frames and closes a subscription that finishes after teardown', async () => {
  let subscribed!: (stop: () => void) => void;
  m.listen.mockReturnValue(new Promise<() => void>((resolve) => { subscribed = resolve; }));
  const events = await import('./appUpdateProgressEvents');
  events.startAppUpdateProgress();
  const receive = m.listen.mock.calls[0][1];
  receive({ payload: { phase: 'checking' } });
  events.stopAppUpdateProgress();
  receive({ payload: { phase: 'installing' } });
  const present = vi.fn();
  events.setAppUpdateProgressHandler(present);
  expect(present).not.toHaveBeenCalled();
  const stop = vi.fn();
  subscribed(stop);
  await vi.waitFor(() => expect(stop).toHaveBeenCalledOnce());
});
