import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  continueThread: vi.fn(),
}));
vi.mock('../scrollState', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../scrollState')>()),
  followContinuedThread: vi.fn(),
}));

import { continueThread } from '../../../api/client';
import { continueStoppedThread } from '../continueStoppedThread';

// Each press emits its own ContinuationRequested, so a second press while the
// first is in flight would start a second resume. The banner's Continue and
// the abort card's both go through this one guard.
describe('continueStoppedThread', () => {
  beforeEach(() => {
    vi.mocked(continueThread).mockReset();
  });

  it('sends one continue for two presses in flight together', async () => {
    let release!: () => void;
    vi.mocked(continueThread).mockReturnValue(new Promise<void>((r) => { release = r; }));
    const first = continueStoppedThread('t-double');
    const second = continueStoppedThread('t-double');
    release();
    expect(await first).toBe(true);
    expect(await second).toBe(false);
    expect(continueThread).toHaveBeenCalledTimes(1);
  });

  it('lets a press after a failure try again', async () => {
    vi.mocked(continueThread).mockRejectedValueOnce(new Error('offline'));
    expect(await continueStoppedThread('t-retry')).toBe(false);
    vi.mocked(continueThread).mockResolvedValueOnce(undefined);
    expect(await continueStoppedThread('t-retry')).toBe(true);
    expect(continueThread).toHaveBeenCalledTimes(2);
  });
});
