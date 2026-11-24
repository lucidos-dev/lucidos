import { describe, it, expect } from 'vitest';
import {
  WORKSPACE_STATE_LABEL,
  workspaceFaultNote,
  workspaceState,
  workspaceStateLabel,
} from './workspaceState';
import type { WorkspaceStatus } from '../api/client/control';

const ws = (health: string, lastError?: string | null): WorkspaceStatus =>
  ({ id: 'w1', name: 'dev', health, last_error: lastError ?? null } as unknown as WorkspaceStatus);

describe('workspaceState', () => {
  it('separates a workspace that was never started from one that broke', () => {
    expect(workspaceState(ws('unhealthy', 'not started'))).toBe('stopped');
    expect(workspaceState(ws('unhealthy', 'engine exited with code 1'))).toBe('unhealthy');
  });

  it('takes the gateway at its word for the other two', () => {
    expect(workspaceState(ws('healthy'))).toBe('healthy');
    expect(workspaceState(ws('booting'))).toBe('booting');
  });
});

describe('workspaceStateLabel', () => {
  it("prefers the engine's own error to the state word", () => {
    expect(workspaceStateLabel(ws('unhealthy', 'port 7420 already in use')))
      .toBe('port 7420 already in use');
  });

  it('falls back to the state word when there is no error to show', () => {
    expect(workspaceStateLabel(ws('healthy'))).toBe(WORKSPACE_STATE_LABEL.healthy);
    expect(workspaceStateLabel(ws('booting'))).toBe(WORKSPACE_STATE_LABEL.booting);
  });
});

describe('workspaceFaultNote', () => {
  it('explains the one state that is a fault', () => {
    expect(workspaceFaultNote(ws('unhealthy', 'engine exited with code 1')))
      .toBe('engine exited with code 1');
  });

  it('names the state when the gateway gives no reason', () => {
    expect(workspaceFaultNote(ws('unhealthy'))).toBe(WORKSPACE_STATE_LABEL.unhealthy);
  });

  it('stays silent for every state the row already explains', () => {
    expect(workspaceFaultNote(ws('unhealthy', 'not started'))).toBeNull();
    expect(workspaceFaultNote(ws('healthy'))).toBeNull();
    expect(workspaceFaultNote(ws('booting'))).toBeNull();
  });
});

it('treats restore as in progress without offering an unhealthy retry', () => {
  expect(workspaceState(ws('restoring'))).toBe('booting');
  expect(workspaceStateLabel(ws('restoring'))).toBe('Restoring…');
  expect(workspaceFaultNote(ws('restoring'))).toBeNull();
});

it('explains a blocked restore failure while keeping restore actions disabled', () => {
  const workspace = ws('restoring', 'Snapshot restore needs attention.');
  expect(workspaceState(workspace)).toBe('booting');
  expect(workspaceStateLabel(workspace)).toBe('Snapshot restore needs attention.');
});
