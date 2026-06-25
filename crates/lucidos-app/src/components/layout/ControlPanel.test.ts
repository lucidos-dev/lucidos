import { describe, it, expect } from 'vitest';
import { currentWorkspaceRefreshState } from './ControlPanel';

describe('currentWorkspaceRefreshState', () => {
  it('no restart, no update — plain refresh tooltip, no dot', () => {
    const { tooltip, showUpdateBadge } = currentWorkspaceRefreshState(false, false);
    expect(tooltip).toBe('Refresh · hold to restart');
    expect(showUpdateBadge).toBe(false);
  });

  it('restart pending, no update — restart-and-apply tooltip, no dot', () => {
    const { tooltip, showUpdateBadge } = currentWorkspaceRefreshState(true, false);
    expect(tooltip).toBe('Refresh · hold to restart & apply changes');
    expect(showUpdateBadge).toBe(false);
  });

  it('update available, no restart — prefixed tooltip, dot shown', () => {
    const { tooltip, showUpdateBadge } = currentWorkspaceRefreshState(false, true);
    expect(tooltip).toBe('Update available · Refresh · hold to restart');
    expect(showUpdateBadge).toBe(true);
  });

  it('both restart pending and update available — both reflected', () => {
    const { tooltip, showUpdateBadge } = currentWorkspaceRefreshState(true, true);
    expect(tooltip).toBe('Update available · Refresh · hold to restart & apply changes');
    expect(showUpdateBadge).toBe(true);
  });
});
