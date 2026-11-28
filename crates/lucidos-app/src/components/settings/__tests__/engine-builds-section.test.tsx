import { describe, it, expect } from 'vitest';
import { switchOutcomeMessage, workspaceBuildState } from '../EngineBuildsSection';
import type { WorkspaceBuild } from '../../../api/client/control';

function row(over: Partial<WorkspaceBuild> = {}): WorkspaceBuild {
  return {
    id: 'dev',
    name: 'Dev',
    running: true,
    build_id: 'abc',
    unanswered: false,
    behind: false,
    ...over,
  };
}

describe('workspaceBuildState', () => {
  it('says a stopped workspace starts on the newest build', () => {
    expect(workspaceBuildState(row({ running: false, build_id: null }), 'abc')).toBe(
      'stopped, starts on the newest build',
    );
  });

  it('tells a running workspace behind from one on the newest build', () => {
    expect(workspaceBuildState(row({ behind: true }), 'new')).toBe('on an older build');
    expect(workspaceBuildState(row(), 'abc')).toBe('on the newest build');
  });

  it('claims the newest build only when the ids match', () => {
    expect(workspaceBuildState(row(), null)).toBe('nothing newer to switch to');
    expect(workspaceBuildState(row({ build_id: 'later' }), 'abc')).toBe('nothing newer to switch to');
  });

  it('says when a running engine did not answer', () => {
    expect(workspaceBuildState(row({ build_id: null, unanswered: true }), 'abc')).toBe('running, not answering');
  });
});

describe('switchOutcomeMessage', () => {
  it('names skipped and failed workspaces', () => {
    const message = switchOutcomeMessage({
      switched: ['dev'],
      skipped: ['eval'],
      failed: [{ id: 'demo', error: 'workspace_restoring' }],
    });
    expect(message.failed).toBe(true);
    expect(message.text).toBe(
      '1 switching to the newest build; 1 left alone (stopped, already switched or not answering); ' +
        `couldn't switch "demo" (workspace_restoring)`,
    );
  });

  it('names no zero count', () => {
    expect(switchOutcomeMessage({ switched: [], skipped: ['dev'], failed: [] }).text).toBe(
      '1 left alone (stopped, already switched or not answering)',
    );
  });

  it('is not a failure when every target switched', () => {
    expect(switchOutcomeMessage({ switched: ['dev'], skipped: [], failed: [] }).failed).toBe(false);
  });
});
