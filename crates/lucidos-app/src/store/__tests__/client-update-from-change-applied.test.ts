import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { updateAvailable, toasts, threadMap, TOAST_AUTO_DISMISS_MS } from '../store';
import { handleThreadEvent } from '../actions/thread-sync';

vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => setTimeout(cb, 0));
vi.stubGlobal('cancelAnimationFrame', (id: number) => clearTimeout(id));

describe('ChangeApplied does NOT eagerly light the update badge', () => {
  // The update badge (updateAvailable) shares the toast's single honest source of
  // truth — the build-id check (syncClientUpdateFromBuild), covered in
  // actions/client-update.test.ts. ChangeApplied must NOT light it: at apply time
  // the rebuilt bundle isn't served yet, so an eager badge would lead the real
  // update and could appear before (or without) the toast. The ChangeApplied
  // handler instead nudges the service worker; the badge+toast surface together
  // once the new /sw.js is genuinely served.
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
    // At ChangeApplied time the rebuilt frontend isn't ready (the build-watch
    // rebuilds over the next few seconds), so a Refresh now would reload the OLD
    // build. The genuine affordance is the SW-driven "New version available →
    // Refresh" toast, which fires only once the rebuild is actually activated.
    const threadId = 't-no-refresh';
    const applyKey = `applying-${threadId}`;

    handleThreadEvent({
      thread_id: threadId,
      seq: 1,
      event: { type: 'ChangeApplied', change_id: 'c-1', requires_restart: false, client_update: true },
      created: '2026-01-01T00:00:00Z',
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
    });
    expect(toasts.value.find(t => t.key === applyKey)).toBeTruthy();

    vi.advanceTimersByTime(TOAST_AUTO_DISMISS_MS);
    expect(toasts.value.find(t => t.key === applyKey)).toBeUndefined();
  });
});
