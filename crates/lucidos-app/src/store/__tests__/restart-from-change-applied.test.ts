import { describe, it, expect, beforeEach } from 'vitest';
import { toasts, restartRequired, restartGroups } from '../store';
import { syncRestartState, restoreRestartState, RESTART_LS_KEY } from '../actions/chat-changes';

const RESTART_FAILURE_TOAST_KEY = 'restart-required';

beforeEach(() => {
  toasts.value = [];
  restartRequired.value = false;
  restartGroups.value = [];
  localStorage.removeItem(RESTART_LS_KEY);
  localStorage.removeItem('lucidos-restart-groups');
});

describe('restart-required state from ChangeApplied requires_restart', () => {

  it('persists restartRequired and shows no toast', () => {
    restartRequired.value = true;
    syncRestartState();

    expect(toasts.value.find(t => t.key === RESTART_FAILURE_TOAST_KEY)).toBeFalsy();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBe('true');
  });

  it('restart state survives page reload via localStorage even if ChangesUpdated was missed', () => {
    restartRequired.value = true;
    syncRestartState();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBe('true');

    restartRequired.value = false;
    toasts.value = [];

    restoreRestartState();

    expect(restartRequired.value).toBe(true);
    expect(toasts.value.find(t => t.key === RESTART_FAILURE_TOAST_KEY)).toBeFalsy();
  });

  it('ChangeApplied with requires_restart=false persists nothing', () => {
    restartRequired.value = false;
    syncRestartState();

    expect(toasts.value.find(t => t.key === RESTART_FAILURE_TOAST_KEY)).toBeFalsy();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBeNull();
  });

  it('restart state persists across a second non-restart ChangeApplied', () => {
    restartRequired.value = true;
    syncRestartState();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBe('true');

    syncRestartState();
    expect(restartRequired.value).toBe(true);
    expect(localStorage.getItem(RESTART_LS_KEY)).toBe('true');
  });

  it('refreshChangesState CAN clear the restart state (REST API path)', () => {
    restartRequired.value = true;
    syncRestartState();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBe('true');

    restartRequired.value = false;
    syncRestartState();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBeNull();
  });
});
