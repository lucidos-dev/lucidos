import { beforeEach, describe, expect, test, vi } from 'vitest';
import type { PresenceCheckPayload } from './presence-pong';

// The order of calls across the two collaborators is the contract: the page
// acts first, then answers. One log records both.
const calls: string[] = [];
const presentMock = vi.fn((_input: unknown) => {
  calls.push('present');
  return 'shown';
});
const submitPongMock = vi.fn((_id: string, _answer: unknown) => {
  calls.push('pong');
});

vi.mock('./devices', () => ({ getDeviceId: () => 'dev-test' }));
vi.mock('./event-stream', () => ({ submitPong: submitPongMock }));
vi.mock('./in-app-notification-toast', () => ({ presentInAppNotification: presentMock }));

const { handlePresenceCheck } = await import('./presence-pong');

function check(over: Partial<PresenceCheckPayload> = {}): PresenceCheckPayload {
  return {
    notification_id: 'n-1',
    title: 'Claude is asking',
    body: 'Pick one',
    thread_id: 't-1',
    event_id: 'e-1',
    app_id: null,
    tap: { kind: 'modal' },
    deadline_ms: 250,
    sent_at_ms: Date.now(),
    ...over,
  };
}

describe('handlePresenceCheck', () => {
  beforeEach(() => {
    calls.length = 0;
    presentMock.mockClear();
    submitPongMock.mockClear();
  });

  test('acts on the notification before it answers', () => {
    handlePresenceCheck(check());
    expect(calls).toEqual(['present', 'pong']);
  });

  test('offers the toast the check carries, aimed at its source', () => {
    handlePresenceCheck(check());
    expect(presentMock).toHaveBeenCalledWith({
      title: 'Claude is asking',
      body: 'Pick one',
      target: { notification: 'n-1', thread: 't-1', event: 'e-1', tap: { kind: 'modal' } },
    });
  });

  test('answers with what the page did, and returns it', () => {
    presentMock.mockReturnValueOnce('not_shown');
    const outcome = handlePresenceCheck(check());
    expect(outcome).toBe('not_shown');
    expect(submitPongMock).toHaveBeenCalledWith('n-1', { device_id: 'dev-test', outcome: 'not_shown' });
  });

  // An iOS PWA buffers SSE while JS is suspended. After a push tap the queued
  // check flushes long past its deadline, when the engine has already pushed.
  test('past its deadline it acts on nothing and answers nothing', () => {
    const outcome = handlePresenceCheck(check({ sent_at_ms: Date.now() - 250 - 1 }));
    expect(outcome).toBeNull();
    expect(presentMock).not.toHaveBeenCalled();
    expect(submitPongMock).not.toHaveBeenCalled();
  });
});
