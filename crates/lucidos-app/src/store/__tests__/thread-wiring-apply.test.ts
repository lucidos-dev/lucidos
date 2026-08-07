import { describe, it, expect, beforeEach } from 'vitest';
import { TS, makeThread } from './thread-wiring-helpers';
import { getCodingAgentWaitingInfo, groupIntoExchanges, type ThreadState } from '../thread-events';
import { flushThreadMap } from '../actions/thread-sync';
import { focusedThreadId, threadMap } from '../store';
import { handleEventWithAgg } from './aggregate-test-helper';

describe('CC follow-up in same thread', () => {
  it('follow-up in CC thread creates exchange in same thread, not a new one', () => {
    const thread = makeThread();
    const map = new Map([['cc-1', thread]]);

    // First CC exchange
    handleEventWithAgg(map, 'cc-1', 1, { type: 'MessageReceived', text: 'fix the bug' }, '2026-03-15T10:00:00Z', undefined);
    handleEventWithAgg(map, 'cc-1', 2, { type: 'SessionStarted', session_id: 'claude-code/20260315' }, TS);
    handleEventWithAgg(map, 'cc-1', 3, { type: 'CodingAgentTextStreamed', text: 'Fixed.' }, TS);
    handleEventWithAgg(map, 'cc-1', 4, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-03-15T10:01:00Z');

    // Follow-up — should be in SAME thread, not a new one
    handleEventWithAgg(map, 'cc-1', 5, { type: 'MessageReceived', text: 'also fix linting' }, '2026-03-15T10:02:00Z', undefined);
    handleEventWithAgg(map, 'cc-1', 6, { type: 'CodingAgentTextStreamed', text: 'Done.' }, TS);
    handleEventWithAgg(map, 'cc-1', 7, { type: 'CodingAgentIdled' } as any, '2026-03-15T10:03:00Z');

    // All events in ONE thread
    expect(map.size).toBe(1);
    expect(thread.events.size).toBe(7);

    // Two exchanges (two MessageReceived)
    const exchanges = [...thread.events.values()].filter(e => e.type === 'MessageReceived');
    expect(exchanges).toHaveLength(2);

    // No redirect response
    const redirects = [...thread.events.values()].filter(e =>
      e.type === 'ResponseGenerated' && (e as any).text?.includes('started a new')
    );
    expect(redirects).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// End-to-end "Apply Now" scenarios — thread events + meta.status updates
// Tests the complete state machine for each apply path.
// ---------------------------------------------------------------------------
describe('Apply Now: Scenario A3 — clean merge (happy path)', () => {
  it('full flow: CC done → Apply Now → ChangeProposed → apply → ChangeApplied → idle', () => {
    const thread = makeThread({ eventsLoaded: true, meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);

    // 1. Claude Code session works and goes idle with changes
    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'fix the bug' }, '2026-01-01T00:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:01Z');
    handleEventWithAgg(map, 't1', 3, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} }, TS);
    handleEventWithAgg(map, 't1', 4, { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' }, TS);
    handleEventWithAgg(map, 't1', 5, { type: 'CodingAgentTextStreamed', text: 'Fixed.' }, TS);
    handleEventWithAgg(map, 't1', 6, { type: 'ResponseGenerated' }, '2026-01-01T00:00:10Z');
    handleEventWithAgg(map, 't1', 7, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:11Z');

    // Thread status: waiting (idle with changes)
    expect(thread.meta.status).toBe('waiting');
    expect(getCodingAgentWaitingInfo(thread.meta)).toEqual({ proposed: true, isExternalRepo: false, requiresRestart: false, applying: false });

    // 3. Backend proposes change, merges, emits ChangeApplied → ChangeProposed + ChangeApplied + SessionEnded
    handleEventWithAgg(map, 't1', 8, { type: 'ChangeProposed', change_id: 'c-1', description: 'Fix', files: ['lib.rs'] } as any, '2026-01-01T00:00:12Z');
    handleEventWithAgg(map, 't1', 9, { type: 'ChangeApplied', change_id: 'c-1', requires_restart: false, path: '' } as any, '2026-01-01T00:00:13Z');
    handleEventWithAgg(map, 't1', 10, { type: 'SessionEnded' }, '2026-01-01T00:00:14Z');

    // Thread status: idle (change resolved, session ended)
    expect(thread.meta.status).toBe('idle');
    expect(getCodingAgentWaitingInfo(thread.meta)).toBeNull();

    // ChangeProposed remains as a step on the CC exchange; ChangeApplied is
    // its own initiator panel exchange (system action with actor + actions).
    const exchanges = groupIntoExchanges(thread.events);
    expect(exchanges).toHaveLength(2);
    const hasProposed = exchanges[0].steps.some(s => s.event.type === 'ChangeProposed');
    expect(hasProposed).toBe(true);
    expect(exchanges[1].userEvent.type).toBe('ChangeApplied');
  });
});

describe('Apply Now: Scenario A1 — hardening not done', () => {
  it('full flow: apply triggers hardening → hardening CC runs → auto-apply → ChangeApplied', () => {
    const thread = makeThread({ eventsLoaded: true, meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);

    // 1. Claude Code session works and goes idle
    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:01Z');
    handleEventWithAgg(map, 't1', 3, { type: 'CodingAgentTextStreamed', text: 'Done.' }, TS);
    handleEventWithAgg(map, 't1', 4, { type: 'ResponseGenerated' }, '2026-01-01T00:00:05Z');
    handleEventWithAgg(map, 't1', 5, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:06Z');
    expect(thread.meta.status).toBe('waiting');

    // 3. Backend sends review follow-up — CC works
    // First need a MessageReceived or CodingAgentUserMessageSent to resume
    handleEventWithAgg(map, 't1', 6, { type: 'CodingAgentUserMessageSent', text: 'Review changes' }, '2026-01-01T00:00:07Z');
    handleEventWithAgg(map, 't1', 7, { type: 'CodingAgentToolCalled', name: 'Read', args: {} }, TS);
    handleEventWithAgg(map, 't1', 8, { type: 'CodingAgentToolResult', name: 'Read', result: 'code...' }, TS);

    // CC resumed work — status=running (from CodingAgentUserMessageSent), no longer waiting
    expect(thread.meta.status).toBe('running');
    expect(getCodingAgentWaitingInfo(thread.meta)).toBeNull();

    // 4. Review finishes, CC idles again
    handleEventWithAgg(map, 't1', 9, { type: 'ResponseGenerated' }, '2026-01-01T00:00:10Z');
    handleEventWithAgg(map, 't1', 10, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:11Z');
    expect(thread.meta.codingAgentProposed).toBe(true);

    // 5. Backend proposes change, merges, emits ChangeApplied + kills CC + SessionEnded
    handleEventWithAgg(map, 't1', 11, { type: 'ChangeProposed', change_id: 'c-1', description: 'Fix', files: ['a.rs'] } as any, '2026-01-01T00:00:12Z');
    handleEventWithAgg(map, 't1', 12, { type: 'ChangeApplied', change_id: 'c-1', requires_restart: false, path: '' } as any, '2026-01-01T00:00:13Z');
    handleEventWithAgg(map, 't1', 13, { type: 'SessionEnded' }, '2026-01-01T00:00:14Z');

    // Thread goes idle
    expect(thread.meta.status).toBe('idle');
    expect(getCodingAgentWaitingInfo(thread.meta)).toBeNull();
  });

  it('review fails → ChangeApplyFailed → thread stays waiting, banner shows error', () => {
    const thread = makeThread({ eventsLoaded: true, meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:01Z');
    handleEventWithAgg(map, 't1', 3, { type: 'ChangeProposed', change_id: 'c-1', description: 'Fix', files: ['a.rs'] } as any, '2026-01-01T00:00:02Z');
    handleEventWithAgg(map, 't1', 4, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:03Z');
    handleEventWithAgg(map, 't1', 5, { type: 'SessionEnded' }, '2026-01-01T00:00:04Z');

    // ChangeApplyFailed arrives (e.g., repo has uncommitted changes)
    handleEventWithAgg(map, 't1', 6, { type: 'ChangeApplyFailed', change_id: 'c-1', error: 'uncommitted changes' } as any, '2026-01-01T00:00:05Z');

    // Thread stays waiting — change is still pending, user can retry
    expect(thread.meta.status).toBe('waiting');

    // ChangeApplyFailed is its own initiator panel exchange (system action,
    // surfaces the error in the body).
    const exchanges = groupIntoExchanges(thread.events);
    expect(exchanges).toHaveLength(2);
    const failedExchange = exchanges[1];
    expect(failedExchange.userEvent.type).toBe('ChangeApplyFailed');
    expect((failedExchange.userEvent as { error?: string }).error).toBe('uncommitted changes');
  });
});

describe('Apply Now: Scenario A4 — no commits to apply (branch already merged)', () => {
  it('ChangeApplied without codingAgentProposed → idle', () => {
    const thread = makeThread({ eventsLoaded: true, meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);

    // 1. Claude Code session works and goes idle with changes
    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:01Z');
    handleEventWithAgg(map, 't1', 3, { type: 'CodingAgentTextStreamed', text: 'Done.' }, TS);
    handleEventWithAgg(map, 't1', 4, { type: 'ResponseGenerated' }, '2026-01-01T00:00:05Z');
    handleEventWithAgg(map, 't1', 5, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:06Z');

    expect(thread.meta.codingAgentProposed).toBe(true);
    expect(thread.meta.status).toBe('waiting');

    // 3. ChangeApplied clears CC flags and sets status to idle
    handleEventWithAgg(map, 't1', 6, { type: 'ChangeApplied', change_id: 'c-1' } as any, '2026-01-01T00:00:07Z');

    // Status becomes idle, CC flags cleared
    expect(thread.meta.status).toBe('idle');
    expect(thread.meta.codingAgentProposed).toBe(false);
    expect(getCodingAgentWaitingInfo(thread.meta)).toBeNull();
  });
});

describe('Apply Now: Scenario A2 — merge conflict', () => {
  it('full flow: apply → conflict → CC resolves → ChangeApplied → idle', () => {
    const thread = makeThread({ eventsLoaded: true, meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);
    const now = new Date();
    const t = (offsetMs: number) => new Date(now.getTime() + offsetMs).toISOString();

    // 1. CC done, idle with changes (fresh timestamps — real-time flow)
    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, t(-20000));
    handleEventWithAgg(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, t(-19000));
    handleEventWithAgg(map, 't1', 3, { type: 'CodingAgentTextStreamed', text: 'Done.' }, TS);
    handleEventWithAgg(map, 't1', 4, { type: 'ChangeProposed', change_id: 'c-1', description: 'Fix', files: ['main.rs'] } as any, t(-17000));
    handleEventWithAgg(map, 't1', 5, { type: 'CodingAgentIdled', has_changes: true } as any, t(-16000));
    handleEventWithAgg(map, 't1', 6, { type: 'SessionEnded' }, t(-15000));

    expect(thread.meta.status).toBe('waiting');

    // 2. Apply triggered → backend detects merge conflict
    handleEventWithAgg(map, 't1', 7, { type: 'MergeConflictDetected', change_id: 'c-1', files: ['main.rs'] } as any, t(-5000));

    // MergeConflictDetected sets codingAgentApplying=true, status stays waiting
    expect(thread.meta.codingAgentApplying).toBe(true);

    // 3. Conflict resolution Claude Code session works
    handleEventWithAgg(map, 't1', 8, { type: 'SessionStarted', session_id: 's2' }, t(-4000));
    // SessionStarted doesn't change status — still waiting
    expect(thread.meta.status).toBe('waiting');
    // CodingAgentPromptSent sets running
    handleEventWithAgg(map, 't1', 8.5, { type: 'CodingAgentPromptSent', text: 'Resolve merge conflict' } as any, t(-3500));
    expect(thread.meta.status).toBe('running');

    handleEventWithAgg(map, 't1', 9, { type: 'CodingAgentToolCalled', name: 'Read', args: {} }, TS);
    handleEventWithAgg(map, 't1', 10, { type: 'CodingAgentToolResult', name: 'Read', result: 'conflict markers...' }, TS);
    handleEventWithAgg(map, 't1', 11, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} }, TS);
    handleEventWithAgg(map, 't1', 12, { type: 'CodingAgentToolResult', name: 'Edit', result: 'resolved' }, TS);
    expect(thread.meta.status).toBe('running');

    // 4. Conflict resolved → ChangeApplied + SessionEnded
    handleEventWithAgg(map, 't1', 13, { type: 'ChangeApplied', change_id: 'c-1', requires_restart: false, path: '' } as any, t(-2000));
    handleEventWithAgg(map, 't1', 14, { type: 'SessionEnded' }, t(-1000));

    expect(thread.meta.status).toBe('idle');
    expect(getCodingAgentWaitingInfo(thread.meta)).toBeNull();

    // Three exchanges: the original user message, the system-spawned merge
    // conflict resolution, and ChangeApplied as its own auditable system action.
    const exchanges = groupIntoExchanges(thread.events);
    expect(exchanges).toHaveLength(3);
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect(exchanges[1].userEvent.type).toBe('MergeConflictDetected');
    expect(exchanges[2].userEvent.type).toBe('ChangeApplied');
  });

  it('conflict resolution fails → ChangeApplyFailed → thread waiting, can retry', () => {
    const thread = makeThread({ eventsLoaded: true, meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:01Z');
    handleEventWithAgg(map, 't1', 3, { type: 'ChangeProposed', change_id: 'c-1', description: 'Fix', files: ['a.rs'] } as any, '2026-01-01T00:00:02Z');
    handleEventWithAgg(map, 't1', 4, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:03Z');
    handleEventWithAgg(map, 't1', 5, { type: 'SessionEnded' }, '2026-01-01T00:00:04Z');

    // Merge conflict, CC tries to resolve but fails
    handleEventWithAgg(map, 't1', 6, { type: 'MergeConflictDetected', change_id: 'c-1', files: ['a.rs'] } as any, '2026-01-01T00:00:05Z');
    handleEventWithAgg(map, 't1', 7, { type: 'ChangeApplyFailed', change_id: 'c-1', error: 'could not resolve conflicts' } as any, '2026-01-01T00:00:06Z');

    // Thread stays waiting — change still pending, user can retry
    expect(thread.meta.status).toBe('waiting');
  });
});

describe('Apply Now: edge cases', () => {
  it('ChangeApplied clears all CC flags → thread goes idle', () => {
    const thread = makeThread({ eventsLoaded: true, meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:01Z');
    handleEventWithAgg(map, 't1', 3, { type: 'ChangeProposed', change_id: 'c-1', description: 'Fix 1', files: ['a.rs'] } as any, '2026-01-01T00:00:02Z');
    // Second round of work
    handleEventWithAgg(map, 't1', 4, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} }, TS);
    handleEventWithAgg(map, 't1', 5, { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' }, TS);
    handleEventWithAgg(map, 't1', 6, { type: 'ChangeProposed', change_id: 'c-2', description: 'Fix 2', files: ['b.rs'] } as any, '2026-01-01T00:00:05Z');
    handleEventWithAgg(map, 't1', 7, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:06Z');
    handleEventWithAgg(map, 't1', 8, { type: 'SessionEnded' }, '2026-01-01T00:00:07Z');

    // Apply changes
    handleEventWithAgg(map, 't1', 9, { type: 'ChangeApplied', change_id: 'c-1', requires_restart: false, path: '' } as any, '2026-01-01T00:00:08Z');

    // ChangeApplied clears all CC flags → idle
    expect(thread.meta.status).toBe('idle');
    expect(thread.meta.codingAgentProposed).toBe(false);
  });

  it('ChangeApplied with requires_restart=true is reflected in events', () => {
    const thread = makeThread({ eventsLoaded: true, meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:01Z');
    handleEventWithAgg(map, 't1', 3, { type: 'ChangeProposed', change_id: 'c-1', description: 'Engine fix', files: ['engine.rs'], requires_restart: true } as any, '2026-01-01T00:00:02Z');
    handleEventWithAgg(map, 't1', 4, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:03Z');
    handleEventWithAgg(map, 't1', 5, { type: 'SessionEnded' }, '2026-01-01T00:00:04Z');
    handleEventWithAgg(map, 't1', 6, { type: 'ChangeApplied', change_id: 'c-1', requires_restart: true, path: '' } as any, '2026-01-01T00:00:05Z');

    expect(thread.meta.status).toBe('idle');

    const allEvents = [...thread.events.values()];
    const applied = allEvents.find(e => e.type === 'ChangeApplied') as any;
    expect(applied.requires_restart).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// ThreadTitleRenamed handling
// ---------------------------------------------------------------------------
describe('ThreadTitleRenamed event handling', () => {
  it('ThreadTitleRenamed updates thread meta title via handleEvent', () => {
    const thread = makeThread();
    thread.meta.title = 'Old Auto Title';
    const map = new Map([['t1', thread]]);

    // Auto-generated title arrives first
    handleEventWithAgg(map, 't1', 1, { type: 'ThreadTitleGenerated', title: 'Auto Title' }, '2026-01-01T00:00:00Z');
    expect(thread.events.get(1)!.type).toBe('ThreadTitleGenerated');

    // User renames the thread
    handleEventWithAgg(map, 't1', 2, { type: 'ThreadTitleRenamed', title: 'My Custom Title' }, '2026-01-01T00:01:00Z');
    expect(thread.events.get(2)!.type).toBe('ThreadTitleRenamed');
  });

  it('ThreadTitleRenamed is persisted (seq present, stored in events map)', () => {
    const thread = makeThread();
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', 42, { type: 'ThreadTitleRenamed', title: 'Renamed' }, '2026-03-18T12:00:00Z');

    expect(thread.events.has(42)).toBe(true);
    const stored = thread.events.get(42)!;
    expect(stored.type).toBe('ThreadTitleRenamed');
    expect((stored as any).title).toBe('Renamed');
    expect(stored.created).toBe('2026-03-18T12:00:00Z');
  });

  it('ThreadTitleRenamed does not affect thread status', () => {
    const thread = makeThread({ eventsLoaded: true });
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'hi' }, '2026-01-01T00:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'ResponseGenerated' }, '2026-01-01T00:00:01Z');
    expect(thread.meta.status).toBe('idle');

    // Renaming doesn't change status
    handleEventWithAgg(map, 't1', 3, { type: 'ThreadTitleRenamed', title: 'New Name' }, '2026-01-01T00:01:00Z');
    expect(thread.meta.status).toBe('idle');
  });

  it('ThreadSaved does not change waiting Claude Code session to running', () => {
    const thread = makeThread({ eventsLoaded: true, meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:01Z');
    handleEventWithAgg(map, 't1', 3, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:02Z');

    expect(thread.meta.status).toBe('waiting');

    // Saving the thread should NOT change status from waiting to running
    handleEventWithAgg(map, 't1', 4, { type: 'ThreadSaved' }, '2026-01-01T00:00:03Z');
    expect(thread.meta.status).toBe('waiting');
  });

  it('ThreadUnsaved does not change waiting Claude Code session to running', () => {
    const thread = makeThread({ eventsLoaded: true, meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:01Z');
    handleEventWithAgg(map, 't1', 3, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:02Z');

    expect(thread.meta.status).toBe('waiting');

    handleEventWithAgg(map, 't1', 4, { type: 'ThreadUnsaved' }, '2026-01-01T00:00:03Z');
    expect(thread.meta.status).toBe('waiting');
  });

  it('ThreadSaved does not change idle thread to running', () => {
    const thread = makeThread({ eventsLoaded: true });
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'hi' }, '2026-01-01T00:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'ResponseGenerated' }, '2026-01-01T00:00:01Z');
    expect(thread.meta.status).toBe('idle');

    handleEventWithAgg(map, 't1', 3, { type: 'ThreadSaved' }, '2026-01-01T00:00:02Z');
    expect(thread.meta.status).toBe('idle');
  });

  it('ThreadTitleRenamed does not create an exchange boundary', () => {
    const thread = makeThread({ eventsLoaded: true });
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'hi' }, '2026-01-01T00:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'ResponseGenerated' }, '2026-01-01T00:00:01Z');
    handleEventWithAgg(map, 't1', 3, { type: 'ThreadTitleRenamed', title: 'New Name' }, '2026-01-01T00:01:00Z');
    handleEventWithAgg(map, 't1', 4, { type: 'MessageReceived', text: 'follow up' }, '2026-01-01T00:02:00Z');

    const exchanges = groupIntoExchanges(thread.events);
    expect(exchanges).toHaveLength(2); // Two MessageReceived = 2 exchanges
  });
});

// ---------------------------------------------------------------------------
// Optimistic Apply Now — apply phase tracking
// The apply phase is managed in the UI and tracks the client-side state.
// Backend status updates via meta.status and meta.cc* flags.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// SSE batching: threadMap signal updates are coalesced via requestAnimationFrame
// ---------------------------------------------------------------------------
describe('SSE batching: flushThreadMap coalesces signal updates', () => {
  beforeEach(() => {
    threadMap.value = new Map();
  });

  it('flushThreadMap creates a new Map reference (triggers signal reactivity)', () => {
    const map = threadMap.value;
    map.set('t1', makeThread());
    const before = threadMap.value;

    flushThreadMap();

    // Signal holds a different Map reference (Preact detects the change)
    expect(threadMap.value).not.toBe(before);
    // But contains the same data
    expect(threadMap.value.size).toBe(1);
    expect(threadMap.value.has('t1')).toBe(true);
  });

  it('multiple in-place mutations followed by one flush produce correct state', () => {
    const map = threadMap.value;
    const t1 = makeThread();
    const t2 = makeThread({ meta: { ...makeThread().meta, id: 'thread-2' } });
    map.set('t1', t1);
    map.set('t2', t2);

    // Simulate rapid SSE events mutating threads in-place
    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'q1' }, '2026-01-01T00:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'ResponseGenerated' }, '2026-01-01T00:00:01Z');
    handleEventWithAgg(map, 't2', 1, { type: 'MessageReceived', text: 'q2' }, '2026-01-01T00:00:02Z');

    // No flush yet — signal still points to same reference
    const beforeFlush = threadMap.value;

    // Single flush after all mutations
    flushThreadMap();

    expect(threadMap.value).not.toBe(beforeFlush);
    expect(threadMap.value.get('t1')!.events.size).toBe(2);
    expect(threadMap.value.get('t2')!.events.size).toBe(1);
    expect(threadMap.value.get('t1')!.meta.status).toBe('idle');
  });
});

// ---------------------------------------------------------------------------
// Backend-authoritative liveness: meta.status from backend
// The backend computes thread status based on actual process state, not
// frontend timestamp heuristics. meta.status is the source of truth.
// ---------------------------------------------------------------------------

describe('Backend-authoritative liveness: meta.status from backend', () => {
  it('meta.status reflects backend-computed state, not timestamp heuristics', () => {
    // Scenario: CC is actively working. The backend reports the thread status
    // via meta.status based on actual process state.
    const thread = makeThread({
      eventsLoaded: true,
      meta: { ...makeThread().meta, channel: 'claude_code', status: 'running' },
    });
    const map = new Map([['t1', thread]]);

    // Events can have stale timestamps, but meta.status is authoritative
    const staleTime = new Date(Date.now() - 120_000).toISOString();

    // Claude Code session started, user sent message, CC is working
    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'fix the bug' }, staleTime);
    handleEventWithAgg(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, staleTime);
    handleEventWithAgg(map, 't1', 3, { type: 'CodingAgentToolCalled', name: 'Read', args: {} }, TS);
    handleEventWithAgg(map, 't1', 4, { type: 'CodingAgentTextStreamed', text: 'Looking...' }, TS);

    // Backend reports status=running — that's the truth, regardless of timestamps
    expect(thread.meta.status).toBe('running');

    // User sent follow-up while CC was working
    handleEventWithAgg(map, 't1', 5, { type: 'CodingAgentUserMessageSent', text: 'check ChangeApplied' }, staleTime);

    // CC resumed work — status stays running
    handleEventWithAgg(map, 't1', 6, { type: 'CodingAgentToolCalled', name: 'Search', args: {} }, staleTime);
    expect(thread.meta.status).toBe('running');
  });

  it('ResponseGenerated transitions running → idle', () => {
    const thread = makeThread({
      eventsLoaded: true,
      meta: { ...makeThread().meta, channel: 'chat', status: 'running' },
    });
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'hi' }, '2026-01-01T00:00:00Z');
    expect(thread.meta.status).toBe('running');

    handleEventWithAgg(map, 't1', 2, { type: 'ResponseGenerated' }, '2026-01-01T00:00:01Z');
    expect(thread.meta.status).toBe('idle');
  });

  it('CodingAgentIdled with has_changes transitions running → waiting', () => {
    const thread = makeThread({
      eventsLoaded: true,
      meta: { ...makeThread().meta, channel: 'claude_code', status: 'running' },
    });
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:01Z');
    expect(thread.meta.status).toBe('running');

    handleEventWithAgg(map, 't1', 3, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:02Z');
    expect(thread.meta.status).toBe('waiting');
    expect(thread.meta.codingAgentProposed).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// No auto-focus on SSE-created threads
// Regression: threads spawned from another workspace or by the agentic loop
// must NOT steal focus. Only explicit user actions (click, sendMessage) focus.
// ---------------------------------------------------------------------------
describe('No auto-focus on SSE-created threads', () => {
  beforeEach(() => {
    threadMap.value = new Map();
    focusedThreadId.value = null;
  });

  /** Create a thread with a specific ID (avoids double makeThread() call). */
  function threadWithId(id: string, extra: Partial<ThreadState['meta']> = {}): ThreadState {
    const t = makeThread();
    t.meta.id = id;
    Object.assign(t.meta, extra);
    return t;
  }

  it('SSE skeleton creation does not change focusedThreadId', () => {
    const map = threadMap.value;
    map.set('spawned-1', threadWithId('spawned-1'));
    handleEventWithAgg(map, 'spawned-1', 1, { type: 'MessageReceived', text: 'task from another workspace' }, '2026-04-13T12:00:00Z');
    threadMap.value = new Map(map);

    expect(focusedThreadId.value).toBeNull();
  });

  it('SSE skeleton creation does not change existing focused thread', () => {
    const map = threadMap.value;
    map.set('focused-thread', threadWithId('focused-thread'));
    focusedThreadId.value = 'focused-thread';

    map.set('spawned-2', threadWithId('spawned-2'));
    handleEventWithAgg(map, 'spawned-2', 1, { type: 'MessageReceived', text: 'spawned task' }, '2026-04-13T12:00:00Z');
    handleEventWithAgg(map, 'spawned-2', 2, { type: 'SessionStarted', session_id: 'cc-spawned' }, '2026-04-13T12:00:01Z');
    threadMap.value = new Map(map);

    expect(focusedThreadId.value).toBe('focused-thread');
  });

  it('CodingAgentThreadSpawned does not auto-focus the spawned CC thread', () => {
    // Regression for commit 0ca048f0: CodingAgentThreadSpawned previously set
    // focusedThreadId to the new CC thread. This must NOT happen.
    const map = threadMap.value;
    map.set('parent-thread', threadWithId('parent-thread'));
    focusedThreadId.value = 'parent-thread';

    handleEventWithAgg(map, 'parent-thread', null, {
      type: 'CodingAgentThreadSpawned',
      cc_thread_id: 'cc-child-1',
      title: 'Fix the bug',
    } as any);
    threadMap.value = new Map(map);

    expect(focusedThreadId.value).toBe('parent-thread');
    expect(localStorage.getItem('lucidos-focused-thread')).not.toBe('cc-child-1');
  });

  it('multiple SSE events on new thread do not steal focus', () => {
    const map = threadMap.value;
    map.set('my-thread', threadWithId('my-thread'));
    focusedThreadId.value = 'my-thread';

    map.set('remote-thread', threadWithId('remote-thread', { channel: 'claude_code' }));
    handleEventWithAgg(map, 'remote-thread', 1, { type: 'MessageReceived', text: 'remote task' }, '2026-04-13T12:00:00Z');
    handleEventWithAgg(map, 'remote-thread', 2, { type: 'SessionStarted', session_id: 'cc-remote' }, '2026-04-13T12:00:01Z');
    handleEventWithAgg(map, 'remote-thread', null, { type: 'CodingAgentTextStreamed', text: 'Working...' } as any);
    handleEventWithAgg(map, 'remote-thread', 3, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} }, TS);
    handleEventWithAgg(map, 'remote-thread', 4, { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' }, TS);
    handleEventWithAgg(map, 'remote-thread', 5, { type: 'ResponseGenerated' }, '2026-04-13T12:01:00Z');
    handleEventWithAgg(map, 'remote-thread', 6, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-04-13T12:01:01Z');
    threadMap.value = new Map(map);

    expect(focusedThreadId.value).toBe('my-thread');
  });
});

