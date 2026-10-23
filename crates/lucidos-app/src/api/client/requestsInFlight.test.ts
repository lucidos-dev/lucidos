// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { requestsInFlight, trackRequest } from './requestsInFlight';

let dones: Array<() => void>;
const track = (url: string) => {
  const done = trackRequest(url);
  dones.push(done);
  return done;
};

beforeEach(() => {
  vi.useFakeTimers();
  dones = [];
});
afterEach(() => {
  dones.forEach((done) => done());
  vi.useRealTimers();
});

describe('requests in flight', () => {
  it('is empty when nothing waits', () => {
    expect(requestsInFlight()).toEqual({ count: 0, oldestMs: null, oldestPaths: [] });
  });

  it('counts a request until it is done, oldest first, without its query', () => {
    const doneA = track('/dev/api/v1/threads?limit=5');
    vi.advanceTimersByTime(1000);
    const doneB = track('/dev/api/v1/apps');
    vi.advanceTimersByTime(3000);

    expect(requestsInFlight()).toEqual({
      count: 2,
      oldestMs: 4000,
      oldestPaths: ['/dev/api/v1/threads', '/dev/api/v1/apps'],
    });

    doneA();
    expect(requestsInFlight()).toMatchObject({ count: 1, oldestMs: 3000, oldestPaths: ['/dev/api/v1/apps'] });
    doneB();
    expect(requestsInFlight().count).toBe(0);
  });

  it('names at most three paths', () => {
    for (const path of ['/a', '/b', '/c', '/d']) {
      track(path);
      vi.advanceTimersByTime(10);
    }
    const seen = requestsInFlight();
    expect(seen.count).toBe(4);
    expect(seen.oldestPaths).toEqual(['/a', '/b', '/c']);
  });
});
