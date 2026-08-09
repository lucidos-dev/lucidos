import { describe, it, expect, beforeEach } from 'vitest';
import { toasts, restartRequired, restartGroups } from '../store';
import { syncRestartToast, restoreRestartToast, RESTART_LS_KEY } from '../actions/chat-changes';

const RESTART_TOAST_KEY = 'restart-required';

beforeEach(() => {
  toasts.value = [];
  restartRequired.value = false;
  restartGroups.value = [];
  localStorage.removeItem(RESTART_LS_KEY);
  localStorage.removeItem('lucidos-restart-groups');
});

describe('restart-required state from ChangeApplied requires_restart', () => {
  // When ChangeApplied SSE arrives with requires_restart=true, the thread-sync
  // handler sets restartRequired and calls syncRestartToast(). Post single-surface
  // decision, this surfaces NO toast — the engine "New version available → Switch"
  // toast is owned by the poll (engine-update.ts) once the rebuild is `ready`. The
  // persisted state (restartRequired + RESTART_LS_KEY) drives the brand badge
  // + restart confirm dialog and must survive a reload.

  it('persists restartRequired and shows no toast', () => {
    restartRequired.value = true;
    syncRestartToast();

    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)).toBeFalsy();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBe('true');
  });

  it('restart state survives page reload via localStorage even if ChangesUpdated was missed', () => {
    restartRequired.value = true;
    syncRestartToast();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBe('true');

    // Simulate page reload (signals reset, toasts cleared)
    restartRequired.value = false;
    toasts.value = [];

    restoreRestartToast();

    expect(restartRequired.value).toBe(true);
    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)).toBeFalsy();
  });

  it('ChangeApplied with requires_restart=false persists nothing', () => {
    restartRequired.value = false;
    syncRestartToast();

    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)).toBeFalsy();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBeNull();
  });

  it('restart state persists across a second non-restart ChangeApplied', () => {
    restartRequired.value = true;
    syncRestartToast();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBe('true');

    // restartRequired stays true (OR logic: once set, stays until engine restarts)
    syncRestartToast();
    expect(restartRequired.value).toBe(true);
    expect(localStorage.getItem(RESTART_LS_KEY)).toBe('true');
  });

  it('refreshChangesState CAN clear the restart state (REST API path)', () => {
    restartRequired.value = true;
    syncRestartToast();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBe('true');

    // Simulate refreshChangesState API response: restart_required=false
    // (e.g. the change was reverted, or the engine restarted)
    restartRequired.value = false;
    syncRestartToast();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBeNull();
  });
});
