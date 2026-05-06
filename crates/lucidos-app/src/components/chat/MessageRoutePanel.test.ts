import { describe, it, expect } from 'vitest';
import {
  resolveOrigin,
  executorExtras,
  resolveThreadLinkTitle,
  renderChannelSection,
  renderAuditSection,
  renderEngineExplainerSection,
} from './MessageRoutePanel';
import type { Exchange, StoredEvent } from '../../store/thread-events';

describe('resolveOrigin', () => {
  it('returns the explicit origin when present on MessageReceived', () => {
    const ev: StoredEvent = {
      type: 'MessageReceived',
      text: 'hi',
      mode: 'human',
      origin: { kind: 'workspace', workspace: 'personal', thread_id: 't1', event_id: 'e1' },
    };
    const o = resolveOrigin(ev);
    expect(o).toEqual({ kind: 'workspace', workspace: 'personal', thread_id: 't1', event_id: 'e1' });
  });

  it('synthesizes a Device origin when only legacy device_id is set', () => {
    const ev: StoredEvent = {
      type: 'MessageReceived',
      text: 'hi',
      mode: 'human',
      device_id: 'dev-1',
      device: 'Chrome',
    };
    expect(resolveOrigin(ev)).toEqual({ kind: 'device', device_id: 'dev-1', label: 'Chrome' });
  });

  it('synthesizes a ThreadLink (direction=parent) origin for agent-mode legacy events', () => {
    const ev: StoredEvent = {
      type: 'MessageReceived',
      text: 'hi',
      mode: 'agent',
      parent_thread_id: 'parent-id',
    };
    expect(resolveOrigin(ev)).toEqual({
      kind: 'thread_link',
      thread_id: 'parent-id',
      spawning_event_id: undefined,
      mode: 'agent',
      direction: 'parent',
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
    const ev: StoredEvent = { type: 'MessageReceived', text: 'hi', mode: 'human' };
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

describe('resolveThreadLinkTitle', () => {
  const parentId = '11111111-1111-1111-1111-111111111111';
  const liveTitle = (title: string | undefined) => (id: string) =>
    id === parentId ? title : undefined;

  it("uses the parent thread's live title when no other title is available", () => {
    // Reproduces the bug: child spawned via run_thread/start_claude_code → MessageReceived
    // emitted with origin: None → frontend synthesizes parent_thread origin with title:
    // undefined. Cached parentThreadTitle is undefined because the thread entered
    // threadMap via the SSE handler (CodingAgentThreadSpawned), which doesn't carry
    // parent metadata. Without this fallback, the popover shows the UUID.
    const result = resolveThreadLinkTitle(
      { kind: 'thread_link', thread_id: parentId, mode: 'agent' },
      undefined,
      liveTitle('Fix interrupt 404 on spawned threads'),
    );
    expect(result).toBe('Fix interrupt 404 on spawned threads');
  });

  it('prefers the live title over the cached parentThreadTitle when both exist (parent renamed)', () => {
    const result = resolveThreadLinkTitle(
      { kind: 'thread_link', thread_id: parentId, mode: 'agent' },
      'Stale cached title',
      liveTitle('Renamed by user'),
    );
    expect(result).toBe('Renamed by user');
  });

  it("ignores the placeholder '...' title and uses the cached fallback", () => {
    const result = resolveThreadLinkTitle(
      { kind: 'thread_link', thread_id: parentId, mode: 'agent' },
      'Cached parent title',
      liveTitle('...'),
    );
    expect(result).toBe('Cached parent title');
  });

  it('falls back to cached title when parent thread is not in threadMap', () => {
    const result = resolveThreadLinkTitle(
      { kind: 'thread_link', thread_id: parentId, mode: 'agent' },
      'Cached title from API',
      liveTitle(undefined),
    );
    expect(result).toBe('Cached title from API');
  });

  it('falls back to UUID only when no source has a title', () => {
    const result = resolveThreadLinkTitle(
      { kind: 'thread_link', thread_id: parentId, mode: 'agent' },
      undefined,
      liveTitle(undefined),
    );
    expect(result).toBe(parentId);
  });

  it('respects an explicit title stamped on the origin (spawn-time fallback when threadMap lacks parent)', () => {
    const result = resolveThreadLinkTitle(
      { kind: 'thread_link', thread_id: parentId, title: 'Title at spawn', mode: 'agent' },
      undefined,
      liveTitle(undefined),
    );
    expect(result).toBe('Title at spawn');
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

  it('reads repo_id from SessionStarted (external repo)', () => {
    const userEvent = stamp(0, { type: 'MessageReceived', text: 'go' });
    const sessionStarted = stamp(1, {
      type: 'SessionStarted',
      session_id: 's1',
      branch: 'claude-code/turn-1',
      repo_id: '550e8400-e29b-41d4-a716-446655440000',
    });
    const exchange: Exchange = { userEvent, userSeq: 1, steps: [{ seq: 2, event: sessionStarted }] };
    const events = new Map<number, StoredEvent>([[1, userEvent], [2, sessionStarted]]);
    const extras = executorExtras(exchange, events);
    expect(extras.repoId).toBe('550e8400-e29b-41d4-a716-446655440000');
  });

  it('returns repoId undefined when SessionStarted has no repo_id (workspace repo)', () => {
    const userEvent = stamp(0, { type: 'MessageReceived', text: 'go' });
    const sessionStarted = stamp(1, { type: 'SessionStarted', session_id: 's1', branch: 'claude-code/x' });
    const exchange: Exchange = { userEvent, userSeq: 1, steps: [{ seq: 2, event: sessionStarted }] };
    const events = new Map<number, StoredEvent>([[1, userEvent], [2, sessionStarted]]);
    const extras = executorExtras(exchange, events);
    expect(extras.repoId).toBeUndefined();
  });

  it('uses the most recent SessionStarted.repo_id for follow-up exchanges (multi-session thread)', () => {
    // Turn 1: workspace repo (no repo_id). Turn 2: switched to external repo. Turn 3 follows up.
    const t1User = stamp(0, { type: 'MessageReceived', text: 'first' });
    const sessA = stamp(1, { type: 'SessionStarted', session_id: 's1', branch: 'wsp-branch' });
    const t2User = stamp(3600, { type: 'MessageReceived', text: 'second' });
    const sessB = stamp(3601, { type: 'SessionStarted', session_id: 's2', branch: 'ext-branch', repo_id: 'repo-uuid-b' });
    const t3User = stamp(4200, { type: 'MessageReceived', text: 'third' });

    const t3: Exchange = { userEvent: t3User, userSeq: 30, steps: [] };
    const events = new Map<number, StoredEvent>([[1, t1User], [2, sessA], [10, t2User], [11, sessB], [30, t3User]]);
    const extras = executorExtras(t3, events);
    expect(extras.repoId).toBe('repo-uuid-b');
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

describe('renderChannelSection', () => {
  it('device origin renders device label', () => {
    const node = renderChannelSection({ kind: 'device', device_id: 'd1', label: 'Chrome on Mac' });
    expect(JSON.stringify(node)).toContain('Chrome on Mac');
  });
  it('api origin renders user-agent', () => {
    const node = renderChannelSection({ kind: 'api', user_agent: 'MyApp/1.0', mode: 'agent' });
    expect(JSON.stringify(node)).toContain('MyApp/1.0');
  });
  it('workspace origin renders workspace name', () => {
    const node = renderChannelSection({
      kind: 'workspace', workspace: 'personal', mode: 'agent',
    });
    expect(JSON.stringify(node)).toContain('personal');
  });
  it('parent_thread origin renders thread title', () => {
    const node = renderChannelSection(
      { kind: 'thread_link', thread_id: 't', title: 'My parent', mode: 'agent' },
      undefined,
      () => 'My parent',
    );
    expect(JSON.stringify(node)).toContain('My parent');
  });
  it('engine origin renders nothing (channel is irrelevant)', () => {
    const node = renderChannelSection({ kind: 'engine', reason: { kind: 'session_recovered' } });
    expect(node).toBeNull();
  });
});

describe('renderAuditSection', () => {
  it('workspace origin with thread/event ids renders them', () => {
    const node = renderAuditSection({
      kind: 'workspace', workspace: 'p', thread_id: 'tid', event_id: 'eid', mode: 'agent',
    });
    const s = JSON.stringify(node);
    expect(s).toContain('tid');
    expect(s).toContain('eid');
  });
  it('parent_thread origin without spawning_event_id renders nothing', () => {
    const node = renderAuditSection({ kind: 'thread_link', thread_id: 't', mode: 'agent' });
    expect(node).toBeNull();
  });
  it('device/api/engine origin renders nothing', () => {
    expect(renderAuditSection({ kind: 'device', device_id: 'd', label: 'L' })).toBeNull();
    expect(renderAuditSection({ kind: 'api' })).toBeNull();
    expect(renderAuditSection({ kind: 'engine', reason: { kind: 'session_recovered' } })).toBeNull();
  });
});

describe('renderEngineExplainerSection', () => {
  it('session_recovered renders explainer text', () => {
    const node = renderEngineExplainerSection({ kind: 'session_recovered' });
    expect(JSON.stringify(node)).toMatch(/auto-resumed/i);
  });
  it('scheduler renders nothing (trigger renderer handles it)', () => {
    expect(renderEngineExplainerSection({ kind: 'scheduler', trigger_id: 't' })).toBeNull();
  });
});
