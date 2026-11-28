import { describe, it, expect, vi } from 'vitest';

describe('connectionStatus initial state', () => {
  it('starts as "connecting" so a refresh never flashes the dot red', async () => {
    vi.resetModules();
    const { connectionStatus } = await import('../store');
    expect(connectionStatus.value).toBe('connecting');
  });
});
