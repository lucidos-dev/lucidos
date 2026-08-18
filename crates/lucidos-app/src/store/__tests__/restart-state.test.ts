import { describe, it, expect, beforeEach } from 'vitest';
import { restartRequired, restartGroups, toasts, engineRestarting, engineRestartNewVersion, activeProgressDialog } from '../store';
import { syncRestartState, restoreRestartState, addRestartGroup, RESTART_LS_KEY, RESTART_IN_FLIGHT_LS_KEY } from '../actions/chat-changes';

const RESTART_FAILURE_TOAST_KEY = 'restart-required';
const RESTART_GROUPS_LS_KEY = 'lucidos-restart-groups';
const LEGACY_REASONS_LS_KEY = 'lucidos-restart-reasons';

beforeEach(() => {
  restartRequired.value = false;
  restartGroups.value = [];
  toasts.value = [];
  engineRestarting.value = false;
  engineRestartNewVersion.value = false;
  localStorage.removeItem(RESTART_LS_KEY);
  localStorage.removeItem(RESTART_IN_FLIGHT_LS_KEY);
  localStorage.removeItem(RESTART_GROUPS_LS_KEY);
  localStorage.removeItem(LEGACY_REASONS_LS_KEY);
});

// The pre-switch "New version available." engine toast was RETIRED (single-surface
// decision, docs/plans/2026-07-01-version-toast-single-surface-and-client-ordering.md):
// the engine "New version available → Switch to new version" surface is owned solely
// by the poll-driven engine-new-version toast (engine-update.ts), which fires only
// once the background rebuild is `ready`. syncRestartState / addRestartGroup do
// bookkeeping only (restartRequired + restartGroups + RESTART_LS_KEY) so the
// brand badge and the restart confirm dialog survive a reload, with no toast.
describe('restart-required state (no pre-switch toast)', () => {
  it('does NOT show a toast when restartRequired is true', () => {
    restartRequired.value = true;
    syncRestartState();
    expect(toasts.value.find(t => t.key === RESTART_FAILURE_TOAST_KEY)).toBeFalsy();
  });

  it('addRestartGroup records state without surfacing a toast', () => {
    addRestartGroup({ threadId: 't1', threadTitle: 'Fix auth', commits: ['feat: a'] });
    expect(restartRequired.value).toBe(true);
    expect(toasts.value.length).toBe(0);
  });

  it('persists restartRequired=true to localStorage', () => {
    restartRequired.value = true;
    syncRestartState();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBe('true');
  });

  it('clears localStorage when restartRequired becomes false', () => {
    restartRequired.value = true;
    syncRestartState();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBe('true');

    restartRequired.value = false;
    syncRestartState();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBeNull();
  });

  it('restoreRestartState restores restartRequired from localStorage (no toast)', () => {
    localStorage.setItem(RESTART_LS_KEY, 'true');
    restartRequired.value = false;
    toasts.value = [];

    restoreRestartState();

    expect(restartRequired.value).toBe(true);
    expect(toasts.value.find(t => t.key === RESTART_FAILURE_TOAST_KEY)).toBeFalsy();
  });

  it('restoreRestartState does nothing when localStorage is empty', () => {
    restoreRestartState();
    expect(restartRequired.value).toBe(false);
    expect(toasts.value.length).toBe(0);
  });

  it('leaves the progress dialog alone while a restart is in flight', () => {
    // Restart in progress: engineRestarting is true and so is restartRequired,
    // which only clears on reconnect. A re-sync (SSE reconnect / new applied
    // change) must not close the dialog by clearing the flag it rides.
    engineRestarting.value = true;
    restartRequired.value = true;
    engineRestartNewVersion.value = true;

    syncRestartState();

    expect(engineRestarting.value).toBe(true);
    expect(activeProgressDialog.value.visible).toBe(true);
    expect(activeProgressDialog.value.title).toBe('Starting new version');
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

  it('clears groups when restartRequired becomes false', () => {
    addRestartGroup({ threadId: 't1', threadTitle: 'Some change', commits: ['feat: x'] });
    restartRequired.value = true;
    syncRestartState();

    restartRequired.value = false;
    syncRestartState();
    expect(restartGroups.value).toEqual([]);
    expect(localStorage.getItem(RESTART_GROUPS_LS_KEY)).toBeNull();
  });

  it('restoreRestartState restores groups from localStorage for the confirm dialog', () => {
    // The groups are needed by the Lucidos menu / SystemPage restart confirm
    // dialog, so they must rehydrate on reload even though no toast is shown.
    const groups = [
      { threadId: 't1', threadTitle: 'Fix auth', commits: ['feat: OAuth'] },
      { threadId: 't2', threadTitle: 'Update API', commits: ['refactor: handler'] },
    ];
    localStorage.setItem(RESTART_LS_KEY, 'true');
    localStorage.setItem(RESTART_GROUPS_LS_KEY, JSON.stringify(groups));

    restoreRestartState();

    expect(restartGroups.value).toEqual(groups);
    expect(toasts.value.find(t => t.key === RESTART_FAILURE_TOAST_KEY)).toBeFalsy();
  });

  it('restoreRestartState ignores legacy "lucidos-restart-reasons" key', () => {
    localStorage.setItem(RESTART_LS_KEY, 'true');
    localStorage.setItem(LEGACY_REASONS_LS_KEY, JSON.stringify(['Old style description']));

    restoreRestartState();

    expect(restartGroups.value).toEqual([]);
    expect(toasts.value.find(t => t.key === RESTART_FAILURE_TOAST_KEY)).toBeFalsy();
  });
});
