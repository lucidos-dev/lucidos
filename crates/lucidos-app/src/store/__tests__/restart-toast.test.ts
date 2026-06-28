import { describe, it, expect, beforeEach } from 'vitest';
import { restartRequired, restartGroups, toasts, showToast, engineVersion, latestEngineVersion, engineRestarting } from '../store';
import { syncRestartToast, restoreRestartToast, addRestartGroup, dismissRestartToast, RESTART_LS_KEY, RESTART_IN_FLIGHT_LS_KEY, RESTART_DISMISSED_FP_LS_KEY } from '../actions/chat-changes';

const RESTART_TOAST_KEY = 'restart-required';
const RESTART_GROUPS_LS_KEY = 'lucidos-restart-groups';
const LEGACY_REASONS_LS_KEY = 'lucidos-restart-reasons';

beforeEach(() => {
  restartRequired.value = false;
  restartGroups.value = [];
  toasts.value = [];
  // The "clicking Restart …" test fires initiateEngineRestart(), which sets
  // engineRestarting=true and (with restartEngine unmocked here) network-fails
  // into the "leave flag set" branch. showToast suppresses every non-restart
  // toast while that flag is set, so reset it or later tests see no toasts.
  engineRestarting.value = false;
  localStorage.removeItem(RESTART_LS_KEY);
  // initiateEngineRestart (fired by the "clicking Restart" test) persists this
  // in-flight marker; clear it so a later test's restoreRestartToast doesn't take
  // the progress-toast branch instead of the pre-restart warning branch.
  localStorage.removeItem(RESTART_IN_FLIGHT_LS_KEY);
  localStorage.removeItem(RESTART_GROUPS_LS_KEY);
  localStorage.removeItem(LEGACY_REASONS_LS_KEY);
  localStorage.removeItem(RESTART_DISMISSED_FP_LS_KEY);
});

describe('restart-required toast persistence', () => {
  it('shows sticky toast when restartRequired is true', () => {
    restartRequired.value = true;
    syncRestartToast();

    const toast = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(toast).toBeTruthy();
    expect(toast!.type).toBe('warning');
    expect(toast!.action).toBeTruthy();
  });

  it('dismisses toast when restartRequired is false', () => {
    restartRequired.value = true;
    syncRestartToast();
    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)).toBeTruthy();

    restartRequired.value = false;
    syncRestartToast();
    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)).toBeFalsy();
  });

  it('persists restartRequired=true to localStorage', () => {
    restartRequired.value = true;
    syncRestartToast();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBe('true');
  });

  it('clears localStorage when restartRequired becomes false', () => {
    restartRequired.value = true;
    syncRestartToast();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBe('true');

    restartRequired.value = false;
    syncRestartToast();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBeNull();
  });

  it('restoreRestartToast re-shows toast from localStorage after signal reset', () => {
    // Simulate: a previous page load set restartRequired and persisted it
    localStorage.setItem(RESTART_LS_KEY, 'true');

    // Simulate page reload: signals are fresh defaults
    restartRequired.value = false;
    toasts.value = [];

    // On startup, restoreRestartToast reads localStorage and re-shows the toast
    restoreRestartToast();

    expect(restartRequired.value).toBe(true);
    const toast = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(toast).toBeTruthy();
    expect(toast!.type).toBe('warning');
  });

  it('restoreRestartToast does nothing when localStorage is empty', () => {
    restoreRestartToast();

    expect(restartRequired.value).toBe(false);
    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)).toBeFalsy();
  });

  it('syncRestartToast skips showToast when warning toast already exists with same message', () => {
    restartRequired.value = true;
    syncRestartToast();
    const toast1 = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(toast1).toBeTruthy();
    const id1 = toast1!.id;

    // Calling again should be a no-op — same toast, same id
    syncRestartToast();
    const toast2 = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(toast2).toBeTruthy();
    expect(toast2!.id).toBe(id1);
    expect(toasts.value.length).toBe(1);
  });

  it('clicking Restart changes toast to info type', () => {
    restartRequired.value = true;
    syncRestartToast();

    const toast = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(toast).toBeTruthy();
    expect(toast!.action).toBeTruthy();

    // Simulate clicking the Restart button
    toast!.action!.onClick();

    // Toast changes to a light info status on the build phase, with a spinner to
    // signal ongoing work. It stays dismissible, since the UI is no longer
    // deactivated during restart.
    const updated = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(updated).toBeTruthy();
    expect(updated!.type).toBe('info');
    expect(updated!.message).toBe('Building the new version…');
    expect(updated!.spinning).toBe(true);
    expect(updated!.dismissable).not.toBe(false);
  });

  it('does not clobber the "Restarting..." status toast while a restart is in flight', () => {
    // Restart in progress: the info status toast owns RESTART_TOAST_KEY and
    // engineRestarting is true, but restartRequired is still true (it only
    // clears on reconnect). A re-sync (SSE reconnect / new applied change) must
    // NOT replace the status toast with the "restart required" warning.
    engineRestarting.value = true;
    restartRequired.value = true;
    showToast('Restarting engine...', 'info', { key: RESTART_TOAST_KEY, showDuringRestart: true });

    syncRestartToast();

    const toast = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(toast).toBeTruthy();
    expect(toast!.type).toBe('info');
    expect(toast!.message).toBe('Restarting engine...');
  });

  it('syncRestartToast re-creates warning toast after Restart changes it to info', () => {
    restartRequired.value = true;
    syncRestartToast();

    // Simulate clicking Restart — toast changes to info type with spinner
    showToast('Restarting engine...', 'info', { key: RESTART_TOAST_KEY, spinning: true });
    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)!.type).toBe('info');

    // If restart fails, syncRestartToast should replace the info toast with warning
    syncRestartToast();
    const warningToast = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(warningToast!.type).toBe('warning');
    expect(warningToast!.action).toBeTruthy();
  });
});

describe('restart groups tracking (thread title + commits)', () => {
  it('addRestartGroup creates a group keyed by threadId with commits', () => {
    addRestartGroup({ threadId: 't1', threadTitle: 'Fix auth middleware', commits: ['feat: add OAuth', 'fix: token refresh'] });

    expect(restartGroups.value).toEqual([
      { threadId: 't1', threadTitle: 'Fix auth middleware', commits: ['feat: add OAuth', 'fix: token refresh'] },
    ]);
    expect(JSON.parse(localStorage.getItem(RESTART_GROUPS_LS_KEY)!)).toEqual(restartGroups.value);
  });

  it('addRestartGroup merges commits into an existing thread group, dedupes, preserves order', () => {
    addRestartGroup({ threadId: 't1', threadTitle: 'Fix auth', commits: ['feat: a', 'fix: b'] });
    addRestartGroup({ threadId: 't1', threadTitle: 'Fix auth', commits: ['fix: b', 'chore: c'] });

    expect(restartGroups.value).toEqual([
      { threadId: 't1', threadTitle: 'Fix auth', commits: ['feat: a', 'fix: b', 'chore: c'] },
    ]);
  });

  it('addRestartGroup updates a stale thread title for the same threadId', () => {
    addRestartGroup({ threadId: 't1', threadTitle: 'Old title', commits: ['feat: a'] });
    addRestartGroup({ threadId: 't1', threadTitle: 'New title', commits: ['feat: b'] });

    expect(restartGroups.value[0].threadTitle).toBe('New title');
  });

  it('addRestartGroup with no commits still records the group (e.g. ff with no real commits)', () => {
    addRestartGroup({ threadId: 't1', threadTitle: 'Refactor', commits: [] });
    expect(restartGroups.value).toEqual([
      { threadId: 't1', threadTitle: 'Refactor', commits: [] },
    ]);
  });

  it('syncRestartToast keeps the message generic regardless of group detail', () => {
    // The toast intentionally hides the per-thread breakdown — the full list
    // lives in the Restart confirm dialog. Keeping the toast short prevents it
    // from dominating the chat view on long sessions.
    addRestartGroup({ threadId: 't1', threadTitle: 'Fix auth middleware', commits: ['feat: OAuth', 'fix: refresh'] });
    addRestartGroup({ threadId: 't2', threadTitle: 'Update scheduler', commits: ['refactor: extract job runner'] });
    restartRequired.value = true;
    syncRestartToast();

    const toast = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(toast).toBeTruthy();
    expect(toast!.message).toBe('Engine restart required to apply changes.');
  });

  it('syncRestartToast falls back to generic message when no groups', () => {
    restartRequired.value = true;
    syncRestartToast();

    const toast = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(toast!.message).toBe('Engine restart required to apply changes.');
  });

  it('clears groups when restartRequired becomes false', () => {
    addRestartGroup({ threadId: 't1', threadTitle: 'Some change', commits: ['feat: x'] });
    restartRequired.value = true;
    syncRestartToast();

    restartRequired.value = false;
    syncRestartToast();
    expect(restartGroups.value).toEqual([]);
    expect(localStorage.getItem(RESTART_GROUPS_LS_KEY)).toBeNull();
  });

  it('restoreRestartToast restores groups from localStorage for the confirm dialog', () => {
    // The toast itself stays generic, but the groups are still needed by the
    // ControlPanel restart confirm dialog, so they must rehydrate on reload.
    const groups = [
      { threadId: 't1', threadTitle: 'Fix auth', commits: ['feat: OAuth'] },
      { threadId: 't2', threadTitle: 'Update API', commits: ['refactor: handler'] },
    ];
    localStorage.setItem(RESTART_LS_KEY, 'true');
    localStorage.setItem(RESTART_GROUPS_LS_KEY, JSON.stringify(groups));

    restoreRestartToast();

    expect(restartGroups.value).toEqual(groups);
    const toast = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(toast!.message).toBe('Engine restart required to apply changes.');
  });

  it('restoreRestartToast ignores legacy "lucidos-restart-reasons" key', () => {
    localStorage.setItem(RESTART_LS_KEY, 'true');
    localStorage.setItem(LEGACY_REASONS_LS_KEY, JSON.stringify(['Old style description']));

    restoreRestartToast();

    expect(restartGroups.value).toEqual([]);
    const toast = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(toast!.message).toBe('Engine restart required to apply changes.');
  });
});

describe('dismissRestartToast — "dismiss for this change"', () => {
  function seedAndDismiss(): void {
    addRestartGroup({ threadId: 't1', threadTitle: 'Fix auth', commits: ['feat: a'] });
    dismissRestartToast();
  }
  const restartToast = () => toasts.value.find(t => t.key === RESTART_TOAST_KEY);

  it('renders a Dismiss action on the warning toast', () => {
    addRestartGroup({ threadId: 't1', threadTitle: 'Fix auth', commits: ['feat: a'] });
    const toast = restartToast();
    expect(toast!.secondaryAction).toBeTruthy();
    expect(toast!.secondaryAction!.label).toBe('Dismiss');
  });

  it('dismissRestartToast removes the toast and persists a fingerprint', () => {
    addRestartGroup({ threadId: 't1', threadTitle: 'Fix auth', commits: ['feat: a'] });
    expect(restartToast()).toBeTruthy();

    dismissRestartToast();

    expect(restartToast()).toBeFalsy();
    expect(localStorage.getItem(RESTART_DISMISSED_FP_LS_KEY)).toBeTruthy();
  });

  it('keeps restartRequired true after dismiss (ControlPanel badge stays)', () => {
    seedAndDismiss();
    expect(restartRequired.value).toBe(true);
  });

  it('syncRestartToast does not re-show toast when fingerprint matches dismissed', () => {
    seedAndDismiss();

    syncRestartToast();
    expect(restartToast()).toBeFalsy();
  });

  it('restoreRestartToast does not re-show toast when fingerprint matches dismissed', () => {
    seedAndDismiss();

    // Simulate page reload: signals reset, localStorage retained.
    toasts.value = [];
    restartRequired.value = false;
    restartGroups.value = [];

    restoreRestartToast();

    expect(restartRequired.value).toBe(true);
    expect(restartToast()).toBeFalsy();
  });

  it('toast reappears when a new thread group is added after dismiss', () => {
    seedAndDismiss();
    expect(restartToast()).toBeFalsy();

    addRestartGroup({ threadId: 't2', threadTitle: 'Update API', commits: ['refactor: handler'] });

    expect(restartToast()).toBeTruthy();
    expect(localStorage.getItem(RESTART_DISMISSED_FP_LS_KEY)).toBeNull();
  });

  it('toast reappears when a new commit is added to an existing dismissed group', () => {
    seedAndDismiss();
    expect(restartToast()).toBeFalsy();

    addRestartGroup({ threadId: 't1', threadTitle: 'Fix auth', commits: ['fix: b'] });

    expect(restartToast()).toBeTruthy();
  });

  it('clears dismissed fingerprint when restartRequired becomes false', () => {
    seedAndDismiss();
    expect(localStorage.getItem(RESTART_DISMISSED_FP_LS_KEY)).toBeTruthy();

    restartRequired.value = false;
    syncRestartToast();

    expect(localStorage.getItem(RESTART_DISMISSED_FP_LS_KEY)).toBeNull();
  });

  it('survives page reload when engine version signals are not yet hydrated', () => {
    // Engine version signals load asynchronously after restoreRestartToast()
    // runs. The fingerprint must not depend on them, otherwise restoring
    // before the health check completes would silently drop the dismissal.
    engineVersion.value = '1.2.3';
    latestEngineVersion.value = '1.2.4';
    seedAndDismiss();

    // Simulate page reload BEFORE the health check has populated versions.
    toasts.value = [];
    restartRequired.value = false;
    restartGroups.value = [];
    engineVersion.value = null;
    latestEngineVersion.value = null;

    restoreRestartToast();

    expect(restartToast()).toBeFalsy();
    expect(localStorage.getItem(RESTART_DISMISSED_FP_LS_KEY)).toBeTruthy();

    engineVersion.value = null;
    latestEngineVersion.value = null;
  });
});
