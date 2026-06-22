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

  it('detects the "Engine restarted" toast Refresh button', () => {
    // Mirrors the toast emitted from connection.ts on reconnect after a restart.
    showToast('Engine restarted', 'success', {
      action: { label: 'Refresh', onClick: () => {} },
    });
    expect(hasRefreshToast()).toBe(true);
  });
});

describe('"New version available" dedup decision', () => {
  // Mirrors the guard in surfaceUpdateToast: skip the toast when a
  // refresh/restart toast is already on screen.
  function maybeShowUpdateToast(): void {
    if (hasRefreshToast()) return;
    showToast('New version available', 'info', {
      key: 'update-available',
      action: { label: 'Refresh', onClick: () => {} },
    });
  }

  it('suppresses the toast when a refresh toast is already showing', () => {
    // The "Engine restarted" reconnect toast carries a Refresh action.
    showToast('Engine restarted', 'success', {
      action: { label: 'Refresh', onClick: () => {} },
    });
    maybeShowUpdateToast();
    expect(toasts.value.some(t => t.message === 'New version available')).toBe(false);
    expect(toasts.value.filter(t => t.action !== undefined)).toHaveLength(1);
  });

  it('suppresses the toast when the restart toast is already showing', () => {
    restartRequired.value = true;
    syncRestartToast();
    maybeShowUpdateToast();
    expect(toasts.value.some(t => t.message === 'New version available')).toBe(false);
  });

  it('still shows the toast when no refresh toast is present', () => {
    maybeShowUpdateToast();
    expect(toasts.value.some(t => t.message === 'New version available')).toBe(true);
  });
});
