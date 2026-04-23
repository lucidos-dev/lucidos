import { describe, it, expect } from 'vitest';
import { resolveOrigin, engineReasonLabel, executorExtras } from './MessageRoutePanel';
import type { Exchange, StoredEvent } from '../../store/thread-events';

describe('resolveOrigin', () => {
  it('returns the explicit origin when present on MessageReceived', () => {
    const ev: StoredEvent = {
      type: 'MessageReceived',
      text: 'hi',
      sender: 'user',
      origin: { kind: 'workspace', workspace: 'personal', thread_id: 't1', event_id: 'e1' },
    };
    const o = resolveOrigin(ev);
    expect(o).toEqual({ kind: 'workspace', workspace: 'personal', thread_id: 't1', event_id: 'e1' });
  });

  it('synthesizes a Device origin when only legacy device_id is set', () => {
    const ev: StoredEvent = {
      type: 'MessageReceived',
      text: 'hi',
      sender: 'user',
      device_id: 'dev-1',
      device: 'Chrome',
    };
    expect(resolveOrigin(ev)).toEqual({ kind: 'device', device_id: 'dev-1', label: 'Chrome' });
  });

  it('synthesizes a ParentThread origin for system-sender legacy events', () => {
    const ev: StoredEvent = {
      type: 'MessageReceived',
      text: 'hi',
      sender: 'system',
      parent_thread_id: 'parent-id',
    };
    expect(resolveOrigin(ev)).toEqual({
      kind: 'parent_thread',
      thread_id: 'parent-id',
      spawning_event_id: undefined,
      mode: 'agent',
    });
  });

  it('returns undefined for non-MessageReceived events without an origin (panel branches separately)', () => {
    const ev: StoredEvent = { type: 'TriggerStarted', trigger_id: 't' };
    expect(resolveOrigin(ev)).toBeUndefined();
  });

  it('extracts engine origin from SessionRecovered events', () => {
    const ev: StoredEvent = {
      type: 'SessionRecovered',
      branch: 'claude-code/x',
      origin: { kind: 'engine', reason: { kind: 'session_recovered' } },
    };
    expect(resolveOrigin(ev)).toEqual({
      kind: 'engine',
      reason: { kind: 'session_recovered' },
    });
  });

  it('extracts engine origin from CodingAgentPromptSent events', () => {
    const ev: StoredEvent = {
      type: 'CodingAgentPromptSent',
      text: '/harden',
      origin: { kind: 'engine', reason: { kind: 'harden_retrigger' } },
    };
    expect(resolveOrigin(ev)?.kind).toBe('engine');
  });

  it('extracts engine origin from TriggerStarted events when present', () => {
    const ev: StoredEvent = {
      type: 'TriggerStarted',
      trigger_id: 'abc',
      origin: { kind: 'engine', reason: { kind: 'scheduler', trigger_id: 'abc', trigger_name: 'nightly' } },
    };
    expect(resolveOrigin(ev)).toEqual({
      kind: 'engine',
      reason: { kind: 'scheduler', trigger_id: 'abc', trigger_name: 'nightly' },
    });
  });

  it('extracts engine origin from ChangeProposed events when present', () => {
    const ev: StoredEvent = {
      type: 'ChangeProposed',
      change_id: 'c1',
      origin: { kind: 'engine', reason: { kind: 'stale_session' } },
    };
    expect(resolveOrigin(ev)).toEqual({
      kind: 'engine',
      reason: { kind: 'stale_session' },
    });
  });

  it('returns undefined when SessionRecovered has no origin field set (legacy DB row)', () => {
    const ev: StoredEvent = { type: 'SessionRecovered', branch: 'x' };
    expect(resolveOrigin(ev)).toBeUndefined();
  });

  it('returns undefined when device_id and parent_thread_id are both missing', () => {
    const ev: StoredEvent = { type: 'MessageReceived', text: 'hi', sender: 'user' };
    expect(resolveOrigin(ev)).toBeUndefined();
  });

  it('surfaces the explicit actor on ChangeApplied', () => {
    const ev: StoredEvent = {
      type: 'ChangeApplied',
      change_id: 'c1',
      actor: { kind: 'device', device_id: 'd1', label: 'Chrome on Mac' },
    };
    expect(resolveOrigin(ev)).toEqual({ kind: 'device', device_id: 'd1', label: 'Chrome on Mac' });
  });

  it('surfaces the actor on ChangeApplyFailed (so the failure has auditability)', () => {
    const ev: StoredEvent = {
      type: 'ChangeApplyFailed',
      change_id: 'c1',
      error: 'merge conflict',
      actor: { kind: 'api', user_agent: 'curl/8' },
    };
    expect(resolveOrigin(ev)).toEqual({ kind: 'api', user_agent: 'curl/8' });
  });
});

describe('engineReasonLabel', () => {
  it('session_recovered → "Auto-resumed after restart"', () => {
    expect(engineReasonLabel({ kind: 'session_recovered' })).toBe('Auto-resumed after restart');
  });

  it('orphan_recovery → "Orphan recovery"', () => {
    expect(engineReasonLabel({ kind: 'orphan_recovery' })).toBe('Orphan recovery');
  });

  it('scheduler with name → "Scheduled · NAME"', () => {
    expect(engineReasonLabel({ kind: 'scheduler', trigger_id: 'x', trigger_name: 'nightly' }))
      .toBe('Scheduled · nightly');
  });

  it('scheduler without name → "Scheduled"', () => {
    expect(engineReasonLabel({ kind: 'scheduler', trigger_id: 'x' })).toBe('Scheduled');
  });

  it('harden_retrigger → "Harden auto-retrigger"', () => {
    expect(engineReasonLabel({ kind: 'harden_retrigger' })).toBe('Harden auto-retrigger');
  });

  it('stale_session → "Stale session cleanup"', () => {
    expect(engineReasonLabel({ kind: 'stale_session' })).toBe('Stale session cleanup');
  });
});

describe('executorExtras', () => {
  /** `at(N)` builds an ISO timestamp N seconds into the test window —
   *  keeps timestamps readable while letting the chronological sort do real work. */
  const at = (seconds: number): string =>
    new Date(Date.UTC(2026, 3, 22, 12, 0, seconds)).toISOString();
  const stamp = <T extends Omit<StoredEvent, 'created'>>(seconds: number, body: T): StoredEvent =>
    ({ ...body, created: at(seconds) }) as StoredEvent;

  it('reads branch from SessionStarted in the same exchange (first CC turn)', () => {
    const userEvent = stamp(0, { type: 'MessageReceived', text: 'go' });
    const sessionStarted = stamp(1, { type: 'SessionStarted', session_id: 's1', branch: 'claude-code/turn-1' });
    const exchange: Exchange = { userEvent, userSeq: 1, steps: [{ seq: 2, event: sessionStarted }] };
    const events = new Map<number, StoredEvent>([[1, userEvent], [2, sessionStarted]]);
    const extras = executorExtras(exchange, events);
    expect(extras.branch).toBe('claude-code/turn-1');
    expect(extras.ccSessionId).toBe('s1');
  });

  it('falls back to earlier SessionStarted for follow-up exchanges in the same CC session', () => {
    // Turn 1: MessageReceived + SessionStarted (branch A)
    // Turn 2: MessageReceived only — no fresh SessionStarted because CC reused the session
    const t1User = stamp(0, { type: 'MessageReceived', text: 'first' });
    const sessionStarted = stamp(1, { type: 'SessionStarted', session_id: 's1', branch: 'claude-code/turn-1' });
    const t2User = stamp(300, { type: 'MessageReceived', text: 'follow up' });
    const t2Tool = stamp(301, { type: 'CodingAgentToolCalled', name: 'Read', args: {} });

    const followUp: Exchange = { userEvent: t2User, userSeq: 10, steps: [{ seq: 11, event: t2Tool }] };
    const events = new Map<number, StoredEvent>([[1, t1User], [2, sessionStarted], [10, t2User], [11, t2Tool]]);
    const extras = executorExtras(followUp, events);
    expect(extras.branch).toBe('claude-code/turn-1');
    expect(extras.ccSessionId).toBe('s1');
  });

  it('uses the most recent SessionStarted when a thread has multiple sessions over time', () => {
    // Two CC sessions back-to-back: branch A then branch B. Follow-up exchange after B
    // must report branch B, not branch A.
    const t1User = stamp(0, { type: 'MessageReceived', text: 'first' });
    const sessA = stamp(1, { type: 'SessionStarted', session_id: 's1', branch: 'branch-A' });
    const t2User = stamp(3600, { type: 'MessageReceived', text: 'second' });
    const sessB = stamp(3601, { type: 'SessionStarted', session_id: 's2', branch: 'branch-B' });
    const t3User = stamp(4200, { type: 'MessageReceived', text: 'third' });

    const t3: Exchange = { userEvent: t3User, userSeq: 30, steps: [] };
    const events = new Map<number, StoredEvent>([[1, t1User], [2, sessA], [10, t2User], [11, sessB], [30, t3User]]);
    const extras = executorExtras(t3, events);
    expect(extras.branch).toBe('branch-B');
    expect(extras.ccSessionId).toBe('s2');
  });

  it('does not leak a future session into an earlier exchange', () => {
    // The first exchange ran on branch A; later the user started a new CC session on
    // branch B. Looking at the first exchange's panel must still show branch A.
    const t1User = stamp(0, { type: 'MessageReceived', text: 'first' });
    const sessA = stamp(1, { type: 'SessionStarted', session_id: 's1', branch: 'branch-A' });
    const t2User = stamp(3600, { type: 'MessageReceived', text: 'second' });
    const sessB = stamp(3601, { type: 'SessionStarted', session_id: 's2', branch: 'branch-B' });

    const t1: Exchange = { userEvent: t1User, userSeq: 1, steps: [{ seq: 2, event: sessA }] };
    const events = new Map<number, StoredEvent>([[1, t1User], [2, sessA], [10, t2User], [11, sessB]]);
    const extras = executorExtras(t1, events);
    expect(extras.branch).toBe('branch-A');
    expect(extras.ccSessionId).toBe('s1');
  });

  it('reads branch from SessionRecovered (engine restart resumes a CC session)', () => {
    const recovered = stamp(0, { type: 'SessionRecovered', branch: 'recovered-branch' });
    const followUp = stamp(60, { type: 'MessageReceived', text: 'continue' });
    const exchange: Exchange = { userEvent: followUp, userSeq: 5, steps: [] };
    const events = new Map<number, StoredEvent>([[1, recovered], [5, followUp]]);
    const extras = executorExtras(exchange, events);
    expect(extras.branch).toBe('recovered-branch');
  });

  it('returns no branch when the thread has no SessionStarted/Recovered events', () => {
    // Pure chat thread (Lucidos, no CC) — no executor branch to show.
    const userEvent = stamp(0, { type: 'MessageReceived', text: 'hi' });
    const exchange: Exchange = { userEvent, userSeq: 1, steps: [] };
    const events = new Map<number, StoredEvent>([[1, userEvent]]);
    const extras = executorExtras(exchange, events);
    expect(extras.branch).toBeUndefined();
    expect(extras.ccSessionId).toBeUndefined();
  });

  it('still extracts permissionMode and context from the current exchange steps only', () => {
    const userEvent = stamp(0, { type: 'MessageReceived', text: 'go' });
    const settings = stamp(1, { type: 'CodingAgentSettingsChanged', permission_mode: 'plan' });
    const thinking = stamp(2, { type: 'Thinking', text: '...', context_tokens: 12345, trimmed: true });
    const exchange: Exchange = {
      userEvent,
      userSeq: 1,
      steps: [{ seq: 2, event: settings }, { seq: 3, event: thinking }],
    };
    const events = new Map<number, StoredEvent>([[1, userEvent], [2, settings], [3, thinking]]);
    const extras = executorExtras(exchange, events);
    expect(extras.permissionMode).toBe('plan');
    expect(extras.contextTokens).toBe(12345);
    expect(extras.contextTrimmed).toBe(true);
  });
});
