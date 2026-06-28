import { describe, it, expect, beforeEach } from 'vitest';
import { toasts, showToast, hasRefreshToast, restartRequired, restartGroups } from '../store';
import { syncRestartToast } from '../actions/chat-changes';

// The "New version available" toast (shown from `surfaceUpdateToast` in
// store/actions/client-update.ts, driven by the build-id check) must not stack on
// top of a toast that already offers the user a way to pick up the new build.
// `hasRefreshToast()` is the predicate the guard uses; these tests pin that it
// detects every real refresh/restart toast. (The end-to-end dedup against the
// real `surfaceUpdateToast` is covered in actions/client-update.test.ts.)

beforeEach(() => {
  toasts.value = [];
  restartRequired.value = false;
  restartGroups.value = [];
});

describe('hasRefreshToast', () => {
  it('is false with no toasts', () => {
    expect(hasRefreshToast()).toBe(false);
  });

  it('is false when only plain (action-less) toasts are present', () => {
    showToast('Saved', 'success');
    showToast('Something failed', 'error');
    expect(hasRefreshToast()).toBe(false);
  });

  it('does NOT count the "Applied …" toast — it never carries an action', () => {
    // The Applied toast is a plain success notification with no Refresh button:
    // at apply time the rebuilt frontend isn't ready, so the refresh affordance
    // is deferred to the SW-driven "New version available" toast.
    showToast('Applied: thing', 'success', { key: 'applying-t1' });
    expect(hasRefreshToast()).toBe(false);
  });

  it('detects the "Engine restart required" toast Restart button', () => {
    restartRequired.value = true;
    syncRestartToast();
    expect(hasRefreshToast()).toBe(true);
  });

  it('does NOT count the action-less "Engine restarted" confirmation', () => {
    // The post-restart "Engine restarted" toast (connection.ts) carries NO action
    // — a pure engine-only restart leaves the client in sync, so it offers no
    // refresh. It must NOT trip hasRefreshToast, otherwise a restart that also
    // rebuilt the client would suppress the genuine "New version available" prompt.
    showToast('Engine restarted', 'success', { autoDismissMs: 5_000 });
    expect(hasRefreshToast()).toBe(false);
  });
});

describe('"New version available" dedup decision', () => {
  // Mirrors the guard in surfaceUpdateToast: skip the toast when a
  // refresh/restart toast is already on screen.
  const UPDATE_MESSAGE = 'New version available — refresh to sync';
  function maybeShowUpdateToast(): void {
    if (hasRefreshToast()) return;
    showToast(UPDATE_MESSAGE, 'info', {
      key: 'update-available',
      action: { label: 'Refresh', onClick: () => {} },
    });
  }

  it('suppresses the toast when the restart toast is already showing', () => {
    restartRequired.value = true;
    syncRestartToast();
    maybeShowUpdateToast();
    expect(toasts.value.some(t => t.message === UPDATE_MESSAGE)).toBe(false);
  });

  it('does NOT suppress when only the action-less "Engine restarted" toast is showing', () => {
    // A restart that also rebuilt the client must still surface the refresh
    // prompt — the action-less confirmation does not count as a refresh path.
    showToast('Engine restarted', 'success', { autoDismissMs: 5_000 });
    maybeShowUpdateToast();
    expect(toasts.value.some(t => t.message === UPDATE_MESSAGE)).toBe(true);
  });

  it('still shows the toast when no refresh toast is present', () => {
    maybeShowUpdateToast();
    expect(toasts.value.some(t => t.message === UPDATE_MESSAGE)).toBe(true);
  });
});
