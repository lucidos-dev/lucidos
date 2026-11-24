import { describe, it, expect, vi, beforeEach } from 'vitest';
import { threads } from './threads';

const asked: string[] = [];

vi.mock('./_fetch', () => ({
  request: (path: string) => {
    asked.push(path);
    return Promise.resolve({ state: 'working', status: 'running', as_of: '2026-11-13T16:43:15Z' });
  },
}));

beforeEach(() => {
  asked.length = 0;
});

describe('threads.liveness', () => {
  it('reads the one thread route, with the id escaped', async () => {
    const got = await threads.liveness('a/b');
    expect(asked).toEqual(['/threads/a%2Fb/liveness']);
    expect(got.state).toBe('working');
  });
});
