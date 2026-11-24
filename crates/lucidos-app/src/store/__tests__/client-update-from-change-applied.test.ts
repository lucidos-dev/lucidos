import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { updateAvailable, toasts, threadMap, TOAST_AUTO_DISMISS_MS } from '../store';
import { handleThreadEvent } from '../actions/thread-sync';
import { makeThreadAggregate } from '../actions/threads-test-helpers';

vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => setTimeout(cb, 0));
vi.stubGlobal('cancelAnimationFrame', (id: number) => clearTimeout(id));

describe('ChangeApplied does NOT eagerly light the update badge', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    updateAvailable.value = false;
    toasts.value = [];
    threadMap.value = new Map();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('leaves updateAvailable false even when client_update=true', () => {
    handleThreadEvent({
      thread_id: 't-badge',
      seq: 1,
      event: { type: 'ChangeApplied', change_id: 'c-1', requires_restart: false, client_update: true },
      created: '2026-01-01T00:00:00Z',
    });
    expect(updateAvailable.value).toBe(false);
  });

  it('leaves updateAvailable false when client_update=false', () => {
    handleThreadEvent({
      thread_id: 't-badge-2',
      seq: 1,
      event: { type: 'ChangeApplied', change_id: 'c-2', requires_restart: false, client_update: false },
      created: '2026-01-01T00:00:00Z',
    });
    expect(updateAvailable.value).toBe(false);
  });
});

describe('Applied toast has no premature Refresh button', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    toasts.value = [];
    threadMap.value = new Map();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('carries NO Refresh action even when client_update=true', () => {
    const threadId = 't-no-refresh';
    const applyKey = `applying-${threadId}`;

    handleThreadEvent({
      thread_id: threadId,
      seq: 1,
      event: { type: 'ChangeApplied', change_id: 'c-1', requires_restart: false, client_update: true },
      created: '2026-01-01T00:00:00Z',
      aggregate: makeThreadAggregate(threadId),
    });
    const toast = toasts.value.find(t => t.key === applyKey);
    expect(toast).toBeTruthy();
    expect(toast?.action).toBeUndefined();
  });

  it('auto-dismisses the Applied toast after TOAST_AUTO_DISMISS_MS', () => {
    const threadId = 't-refresh-timer';
    const applyKey = `applying-${threadId}`;

    handleThreadEvent({
      thread_id: threadId,
      seq: 1,
      event: { type: 'ChangeApplied', change_id: 'c-1', requires_restart: false, client_update: true },
      created: '2026-01-01T00:00:00Z',
      aggregate: makeThreadAggregate(threadId),
    });
    expect(toasts.value.find(t => t.key === applyKey)).toBeTruthy();

    vi.advanceTimersByTime(TOAST_AUTO_DISMISS_MS);
    expect(toasts.value.find(t => t.key === applyKey)).toBeUndefined();
  });
});
