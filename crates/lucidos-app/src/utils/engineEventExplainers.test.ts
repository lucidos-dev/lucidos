import { describe, it, expect } from 'vitest';
import {
  describeAbortCause,
  describeCancelCause,
  describeContinuationReason,
  describeEngineReason,
} from './engineEventExplainers';
import {
  CONTINUATION_AUTO_RECOVERY_REASON,
  CONTINUATION_AUTO_RESUME_AFTER_API_ERROR_REASON,
  CONTINUATION_AUTO_RESUME_AFTER_SWITCH_REASON,
  CONTINUATION_EFFORT_RESTART_REASON,
  CONTINUATION_USER_CLICKED_REASON,
  continuationCause,
  continuationStartedSummary,
  type AbortCause,
  type CancelCause,
} from '../store/thread-events';

const ABORT_CAUSE_FLAGS = {
  safety_net: true,
  engine_shutdown: true,
  recovery_after_restart: true,
  process_killed: true,
  stale_settle: true,
  session_dropped: true,
  snapshot_restored: true,
  unknown: true,
} satisfies Record<AbortCause, true>;
const ABORT_CAUSES = Object.keys(ABORT_CAUSE_FLAGS) as AbortCause[];

const CANCEL_CAUSE_FLAGS = {
  user_stop: true,
  user_action: true,
  superseded_by_followup: true,
  effort_restart: true,
  refused: true,
  hold: true,
  unknown: true,
} satisfies Record<CancelCause, true>;
const CANCEL_CAUSES = Object.keys(CANCEL_CAUSE_FLAGS) as CancelCause[];

describe('describeEngineReason', () => {
  it('returns explainer for session_recovered', () => {
    expect(describeEngineReason({ kind: 'session_recovered' }))
      .toMatch(/picked it back up/i);
  });
  it('returns explainer for orphan_recovery', () => {
    expect(describeEngineReason({ kind: 'orphan_recovery' }))
      .toMatch(/restarted.*offered it as a change, marked unfinished/i);
  });
  it('returns explainer for harden_retrigger', () => {
    expect(describeEngineReason({ kind: 'harden_retrigger' }))
      .toMatch(/harden/i);
  });
  it('returns explainer for stale_session', () => {
    expect(describeEngineReason({ kind: 'stale_session' }))
      .toMatch(/stopped running.*offered its work/i);
  });
  it('returns explainer for archived_branch_work', () => {
    expect(describeEngineReason({ kind: 'archived_branch_work' }))
      .toMatch(/kept it on the thread’s branch.*unarchive/i);
  });
  it('returns explainer for merge_conflict', () => {
    expect(describeEngineReason({ kind: 'merge_conflict' }))
      .toMatch(/merge conflict/i);
  });
  it('returns explainer for missing_hardening', () => {
    expect(describeEngineReason({ kind: 'missing_hardening' }))
      .toMatch(/harden/i);
  });
  it('returns explainer for missing_plan', () => {
    expect(describeEngineReason({ kind: 'missing_plan' }))
      .toMatch(/implementation plan/i);
  });
  it('returns explainer for context_ceiling', () => {
    expect(describeEngineReason({ kind: 'context_ceiling' }))
      .toMatch(/context ceiling/i);
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
  it('names the trigger behind a scheduled message, and never its id', () => {
    expect(describeEngineReason({ kind: 'scheduler', trigger_id: 't-1', trigger_name: 'Nightly digest' }))
      .toMatch(/“Nightly digest” ran on its schedule/);
    const unnamed = describeEngineReason({ kind: 'scheduler', trigger_id: 't-1' });
    expect(unnamed).toMatch(/A trigger ran on its schedule/);
    expect(unnamed).not.toContain('t-1');
  });
  it('names the actor email a trigger fire carries', () => {
    expect(describeEngineReason({
      kind: 'scheduler',
      trigger_id: 't-1',
      trigger_name: 'Ticket intake',
      actor_email: 'sam@example.com',
    })).toMatch(/action by sam@example\.com/);
  });

  it('tells a timed-out wait as a story', () => {
    const text = describeEngineReason({
      kind: 'event_wait',
      outcome: 'expired',
      watched: ['BenchSlotReleased'],
      wait_reason: 'waiting for the bench slot',
    });
    expect(text).toBe(
      'The agent asked Lucidos to tell it when “bench slot released” happened, because: '
      + 'waiting for the bench slot. Nothing happened before its deadline, so Lucidos told '
      + 'the agent, to report back.',
    );
  });

  it('tells an arrived wait, naming every watched event in plain words', () => {
    const text = describeEngineReason({
      kind: 'event_wait',
      outcome: 'delivered',
      watched: ['ChangeProposed', 'CodingAgentIdled'],
      wait_reason: 'the child to finish',
    });
    expect(text).toMatch(/“change proposed or coding agent stopped working” happened/);
    expect(text).toMatch(/It happened, so Lucidos told the agent/);
  });

  it('drops the clauses an older re-entry did not record', () => {
    const text = describeEngineReason({ kind: 'event_wait', outcome: 'expired', watched: [], wait_reason: '' });
    expect(text).toBe(
      'The agent asked Lucidos to tell it when an event happened. Nothing happened before '
      + 'its deadline, so Lucidos told the agent, to report back.',
    );
  });
});

const ENGINE_INTERNALS = /marker|orphan|safety net|event loop|Thread Queue|driver task|ContinuationRequested|`|session/i;

describe('every explainer is plain', () => {
  it('names no engine internal and carries no markdown', () => {
    const texts = [
      ...ABORT_CAUSES.map(describeAbortCause),
      ...CANCEL_CAUSES.map(describeCancelCause),
      ...(['continuation_started', 'session_recovered', 'orphan_recovery', 'harden_retrigger',
        'stale_session', 'archived_branch_work', 'unmerged_integrated_work', 'successor_residue', 'merge_conflict', 'main_check_red', 'main_check_flaky',
        'missing_hardening', 'missing_plan', 'approved_plan_unstarted', 'model_substituted', 'context_ceiling'] as const)
        .map(kind => describeEngineReason({ kind })),
    ];
    for (const text of texts) expect(text).not.toMatch(ENGINE_INTERNALS);
  });

  it('never names one agent product for every thread', () => {
    for (const cause of CANCEL_CAUSES) expect(describeCancelCause(cause)).not.toMatch(/Claude Code|Codex/);
    for (const cause of ABORT_CAUSES) expect(describeAbortCause(cause)).not.toMatch(/Claude Code|Codex/);
  });
});

describe('describeAbortCause', () => {
  it('explains safety_net by its two cases, in plain words', () => {
    const text = describeAbortCause('safety_net');
    expect(text).toMatch(/stopped without finishing/i);
    expect(text).toMatch(/waited a minute/i);
  });
  it('mentions shutdown for engine_shutdown', () => {
    expect(describeAbortCause('engine_shutdown')).toMatch(/shut down|restarted/i);
  });
  it('explains recovery_after_restart as a deliberate hold, not an impossibility', () => {
    const text = describeAbortCause('recovery_after_restart');
    expect(text).toMatch(/restart/i);
    expect(text).toMatch(/Continue/);
    expect(text).not.toMatch(/could not be resumed/i);
  });
  it('says the agent crashed for process_killed', () => {
    expect(describeAbortCause('process_killed')).toMatch(/ended unexpectedly/i);
  });
  it('says nothing was lost for stale_settle', () => {
    expect(describeAbortCause('stale_settle')).toMatch(/nothing was working on it.*No work was lost/i);
  });
  it('falls back for unknown / undefined', () => {
    expect(describeAbortCause('unknown')).toMatch(/did not record why/i);
    expect(describeAbortCause(undefined)).toMatch(/did not record why/i);
  });
  it('returns a non-empty string for every AbortCause variant', () => {
    for (const cause of ABORT_CAUSES) {
      const text = describeAbortCause(cause);
      expect(text.length).toBeGreaterThan(0);
    }
  });
});

describe('describeContinuationReason', () => {
  it('names the Switch that stopped the response', () => {
    expect(describeContinuationReason(CONTINUATION_AUTO_RESUME_AFTER_SWITCH_REASON))
      .toMatch(/chose Switch on the new version/);
  });

  it('does not claim an engine restart for auto_recovery_after_hang', () => {
    const text = describeContinuationReason(CONTINUATION_AUTO_RECOVERY_REASON);
    expect(text).toMatch(/stopped responding|stray signal/i);
    expect(text).toMatch(/nothing restarted/i);
  });

  it('attributes a user-clicked Continue to the user', () => {
    expect(describeContinuationReason(CONTINUATION_USER_CLICKED_REASON)).toMatch(/^You pressed Continue/);
  });

  it('explains an api-error auto-resume without claiming a restart', () => {
    const text = describeContinuationReason(CONTINUATION_AUTO_RESUME_AFTER_API_ERROR_REASON);
    expect(text).not.toBeNull();
    expect(text).toMatch(/dropped/i);
    expect(text).toMatch(/nothing restarted/i);
    expect(text).toMatch(/few times|at most/i);
  });

  it('names which interruption the api-error resume recovered from', () => {
    const label = continuationStartedSummary(CONTINUATION_AUTO_RESUME_AFTER_API_ERROR_REASON, undefined);
    expect(label).toMatch(/connection/i);
    expect(label).not.toMatch(/restart/i);
  });

  it('names which interruption the hang recovery recovered from', () => {
    const label = continuationStartedSummary(CONTINUATION_AUTO_RECOVERY_REASON, undefined);
    expect(label).toMatch(/stopped responding/i);
    expect(label).not.toMatch(/restart/i);
  });

  it('gives the two auto-resume reasons distinguishable labels', () => {
    expect(continuationStartedSummary(CONTINUATION_AUTO_RESUME_AFTER_API_ERROR_REASON, undefined))
      .not.toBe(continuationStartedSummary(CONTINUATION_AUTO_RECOVERY_REASON, undefined));
  });

  it('says an effort restart came from the effort pick, not an engine restart', () => {
    expect(describeContinuationReason(CONTINUATION_EFFORT_RESTART_REASON)).toMatch(/^You picked a new reasoning effort/);
    expect(continuationStartedSummary(CONTINUATION_EFFORT_RESTART_REASON, undefined))
      .toBe('Restarted at the new reasoning effort');
    expect(continuationCause(CONTINUATION_EFFORT_RESTART_REASON, undefined)).toBe('You changed effort');
  });

  it('returns null for an unrecorded or unrecognized reason rather than inventing one', () => {
    expect(describeContinuationReason(undefined)).toBeNull();
    expect(describeContinuationReason('answered_after_idle')).toBeNull();
  });
});

describe('describeCancelCause', () => {
  it('mentions the Cancel button for user_stop', () => {
    expect(describeCancelCause('user_stop')).toMatch(/cancel/i);
  });
  it('mentions apply/discard/archive for user_action', () => {
    expect(describeCancelCause('user_action')).toMatch(/appl|discard|archiv/i);
  });
  it('names the effort pick for effort_restart', () => {
    const text = describeCancelCause('effort_restart');
    expect(text).toMatch(/reasoning effort/);
    expect(text).not.toMatch(/follow-up/);
  });
  it('falls back for unknown / undefined', () => {
    expect(describeCancelCause('unknown')).toMatch(/did not record how/i);
    expect(describeCancelCause(undefined)).toMatch(/did not record how/i);
  });
  it('attributes every cause but a hold to the user ("You")', () => {
    for (const cause of CANCEL_CAUSES.filter((c) => c !== 'hold')) {
      expect(describeCancelCause(cause)).toMatch(/^You\b/);
    }
  });

  it('names no actor for a hold, which an agent may have placed', () => {
    expect(describeCancelCause('hold')).toMatch(/^This thread was put on hold/);
  });
});
