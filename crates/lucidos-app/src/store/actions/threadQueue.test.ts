import { describe, it, expect, vi } from 'vitest';

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  getThreadQueue: vi.fn(),
}));

import { getThreadQueue } from '../../api/client';
import { threadQueueHolds } from './threadQueue';

describe('threadQueueHolds', () => {
  it('rejects when the queue cannot be read, so a caller never takes that for not queued', async () => {
    vi.mocked(getThreadQueue).mockRejectedValueOnce(new Error('engine unreachable'));

    await expect(threadQueueHolds('t1')).rejects.toThrow('engine unreachable');
  });
});
