import { describe, it, expect } from 'vitest';
import { describeAbortCause, describeCancelCause, describeEngineReason } from './engineEventExplainers';
import type { AbortCause, CancelCause } from '../store/thread-events';

describe('describeEngineReason', () => {
  it('returns explainer for session_recovered', () => {
    expect(describeEngineReason({ kind: 'session_recovered' }))
      .toMatch(/auto-resumed/i);
  });
  it('returns explainer for orphan_recovery', () => {
    expect(describeEngineReason({ kind: 'orphan_recovery' }))
      .toMatch(/orphaned/i);
  });
  it('returns explainer for harden_retrigger', () => {
    expect(describeEngineReason({ kind: 'harden_retrigger' }))
      .toMatch(/harden/i);
  });
  it('returns explainer for stale_session', () => {
    expect(describeEngineReason({ kind: 'stale_session' }))
      .toMatch(/stale/i);
  });
  it('returns explainer for merge_conflict', () => {
    expect(describeEngineReason({ kind: 'merge_conflict' }))
      .toMatch(/conflict/i);
  });
  it('returns explainer for missing_hardening', () => {
    expect(describeEngineReason({ kind: 'missing_hardening' }))
      .toMatch(/harden/i);
  });
  it('returns explainer for plugin_auto_update', () => {
    expect(describeEngineReason({
      kind: 'plugin_auto_update',
      plugin_id: 'browser-learning',
      marketplace_id: 'core',
      marketplace_name: 'Core',
    }))
      .toMatch(/browser-learning.*Core/i);
  });
  it('returns null for scheduler (handled by trigger renderer)', () => {
    expect(describeEngineReason({ kind: 'scheduler', trigger_id: 't' }))
      .toBeNull();
  });
});

describe('describeAbortCause', () => {
  it('explains safety_net as a non-watchdog event-loop crash now that the watchdog auto-resumes', () => {
    // The 10-min watchdog path used to land here too — now it emits
    // ContinuationRequested{auto_recovery_after_hang} and the user never sees
    // safety_net for the hung-API-call case. The remaining cases are
    // genuine Claude Code session / driver failures.
    const text = describeAbortCause('safety_net');
    expect(text).toMatch(/crash|driver|event loop/i);
    expect(text).toMatch(/auto-resume|ContinuationRequested/i);
  });
  it('mentions shutdown for engine_shutdown', () => {
    expect(describeAbortCause('engine_shutdown')).toMatch(/shut down|restarted/i);
  });
  it('mentions recovery for recovery_after_restart', () => {
    expect(describeAbortCause('recovery_after_restart')).toMatch(/recover/i);
  });
  it('mentions the session for process_killed', () => {
    expect(describeAbortCause('process_killed')).toMatch(/session/i);
  });
  it('mentions cleanup for stale_settle', () => {
    expect(describeAbortCause('stale_settle')).toMatch(/clean|stuck/i);
  });
  it('falls back for unknown / undefined', () => {
    expect(describeAbortCause('unknown')).toMatch(/cause not recorded/i);
    expect(describeAbortCause(undefined)).toMatch(/cause not recorded/i);
  });
  it('returns a non-empty string for every AbortCause variant', () => {
    const causes: AbortCause[] = [
      'safety_net',
      'engine_shutdown',
      'recovery_after_restart',
      'process_killed',
      'stale_settle',
      'unknown',
    ];
    for (const cause of causes) {
      const text = describeAbortCause(cause);
      expect(text.length).toBeGreaterThan(0);
    }
  });
});

describe('describeCancelCause', () => {
  it('mentions the Cancel button for user_stop', () => {
    expect(describeCancelCause('user_stop')).toMatch(/cancel/i);
  });
  it('mentions apply/discard/archive for user_action', () => {
    expect(describeCancelCause('user_action')).toMatch(/appl|discard|archiv/i);
  });
  it('falls back for unknown / undefined', () => {
    expect(describeCancelCause('unknown')).toMatch(/cause not recorded/i);
    expect(describeCancelCause(undefined)).toMatch(/cause not recorded/i);
  });
  it('attributes every cause to the user ("You")', () => {
    const causes: CancelCause[] = ['user_stop', 'user_action', 'unknown'];
    for (const cause of causes) {
      expect(describeCancelCause(cause)).toMatch(/^You\b/);
    }
  });
});
