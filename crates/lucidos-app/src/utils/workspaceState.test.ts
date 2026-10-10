/**
 * The one reading of a workspace's health, and the sentence a faulty one owes
 * the user.
 *
 * The distinction the whole module exists for is the pair the gateway cannot
 * tell apart on its own: a workspace that was never started answers `unhealthy`
 * with "not started", which is a calm idle, not a failure. Every surface that
 * draws a dot for a workspace reads this, so the two must not disagree, and
 * nothing may grow a red explanation for a workspace that is simply off.
 */
import { describe, it, expect } from 'vitest';
import {
  WORKSPACE_STATE_LABEL,
  workspaceFaultNote,
  workspaceState,
  workspaceStateLabel,
} from './workspaceState';
import type { WorkspaceStatus } from '../api/client/control';

/** A gateway status row, with only the fields these rules read set. */
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
    // The state the picker draws in red, and the only one where the row owes an
    // explanation rather than a label.
    expect(workspaceFaultNote(ws('unhealthy', 'engine exited with code 1')))
      .toBe('engine exited with code 1');
  });

  it('names the state when the gateway gives no reason', () => {
    expect(workspaceFaultNote(ws('unhealthy'))).toBe(WORKSPACE_STATE_LABEL.unhealthy);
  });

  it('stays silent for every state the row already explains', () => {
    // A stopped workspace is the trap: it reaches us as `unhealthy` with "not
    // started", and a red sentence under a workspace the user switched off
    // would be a fault reported where there is none.
    expect(workspaceFaultNote(ws('unhealthy', 'not started'))).toBeNull();
    expect(workspaceFaultNote(ws('healthy'))).toBeNull();
    expect(workspaceFaultNote(ws('booting'))).toBeNull();
  });
});
