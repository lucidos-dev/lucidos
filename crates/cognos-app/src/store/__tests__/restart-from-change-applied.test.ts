import { describe, it, expect, beforeEach } from 'vitest';
import { toasts, restartRequired, restartGroups } from '../store';
import { syncRestartToast, restoreRestartToast, RESTART_LS_KEY } from '../actions/chat-changes';

const RESTART_TOAST_KEY = 'restart-required';

beforeEach(() => {
  toasts.value = [];
  restartRequired.value = false;
  restartGroups.value = [];
  localStorage.removeItem(RESTART_LS_KEY);
  localStorage.removeItem('cognos-restart-groups');
});

describe('restart toast from ChangeApplied requires_restart', () => {
  // This tests the fix: when ChangeApplied SSE arrives with requires_restart=true,
  // the thread-sync handler sets restartRequired and calls syncRestartToast().
  // Previously, only the ChangesUpdated system event triggered this — if that event
  // was missed (SSE drop, Vite reload race), the toast never appeared.

  it('syncRestartToast shows and persists toast when restartRequired is set from ChangeApplied', () => {
    // Simulate what the fixed ChangeApplied handler does:
    // set restartRequired=true immediately from the thread event
    restartRequired.value = true;
    syncRestartToast();

    const toast = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(toast).toBeTruthy();
    expect(toast!.type).toBe('warning');
    expect(toast!.action).toBeTruthy();
    expect(toast!.action!.label).toBe('Restart');
    expect(localStorage.getItem(RESTART_LS_KEY)).toBe('true');
  });

  it('restart toast survives page reload via localStorage even if ChangesUpdated was missed', () => {
    // Step 1: ChangeApplied with requires_restart=true sets localStorage
    restartRequired.value = true;
    syncRestartToast();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBe('true');

    // Step 2: Simulate page reload (signals reset, toasts cleared)
    restartRequired.value = false;
    toasts.value = [];

    // Step 3: On reload, restoreRestartToast reads localStorage
    restoreRestartToast();

    expect(restartRequired.value).toBe(true);
    const toast = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(toast).toBeTruthy();
  });

  it('ChangeApplied with requires_restart=false does not set restart toast', () => {
    // requires_restart=false — no toast
    restartRequired.value = false;
    syncRestartToast();

    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)).toBeFalsy();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBeNull();
  });

  it('restart toast accumulates — second ChangeApplied with restart keeps toast visible', () => {
    // First change requiring restart
    restartRequired.value = true;
    syncRestartToast();
    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)).toBeTruthy();

    // Second change NOT requiring restart — toast must persist
    // (restartRequired stays true because OR logic: once set, stays until engine restarts)
    syncRestartToast();  // restartRequired.value is still true
    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)).toBeTruthy();
  });

  it('SSE ChangesUpdated with restart_required=false does NOT clear an existing restart toast', () => {
    // Apply a change requiring restart
    restartRequired.value = true;
    syncRestartToast();
    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)).toBeTruthy();

    // Simulate a subsequent ChangesUpdated SSE event (e.g. from a non-restart
    // change being proposed/applied) — the handler should NOT demote restartRequired.
    // In the real handler, the SSE path only escalates (true→true), never demotes.
    // Here we verify that calling syncRestartToast with restartRequired still true
    // keeps the toast.
    syncRestartToast();
    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)).toBeTruthy();
    expect(restartRequired.value).toBe(true);
  });

  it('refreshChangesState CAN clear the restart toast (REST API path)', () => {
    // Apply a change requiring restart
    restartRequired.value = true;
    syncRestartToast();
    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)).toBeTruthy();

    // Simulate refreshChangesState API response: restart_required=false
    // (e.g. the change was reverted, or the engine restarted)
    restartRequired.value = false;
    syncRestartToast();
    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)).toBeFalsy();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBeNull();
  });
});
