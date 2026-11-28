import { describe, it, expect, beforeEach, vi } from 'vitest';
import { pinnedApps } from '../store';

vi.mock('../../api/client', () => ({
  getPinnedAppUis: vi.fn(),
  pinAppApi: vi.fn(),
  unpinAppApi: vi.fn(),
}));
vi.mock('./devices', () => ({ getDeviceId: () => 'device-1' }));

const { pinApp, removePinnedAppLocal } = await import('./pinnedApps');
const { pinAppApi } = await import('../../api/client');

function rejectable(): { promise: Promise<void>; reject: (e: Error) => void } {
  let reject!: (e: Error) => void;
  const promise = new Promise<void>((_, r) => { reject = r; });
  return { promise, reject };
}

describe('a failed pin reverts from the live list, not a snapshot', () => {
  beforeEach(() => {
    localStorage.clear();
    pinnedApps.value = { status: 'loaded', data: [{ app_id: 'ledger' }, { app_id: 'habit-tracker' }] };
  });

  it('keeps an entry an SSE removed while the pin was in flight', async () => {
    const pin = rejectable();
    (pinAppApi as ReturnType<typeof vi.fn>).mockReturnValueOnce(pin.promise);

    const pending = pinApp('chess');
    removePinnedAppLocal('ledger');
    pin.reject(new Error('engine offline'));
    await pending;

    expect(pinnedApps.value).toEqual({ status: 'loaded', data: [{ app_id: 'habit-tracker' }] });
  });
});
