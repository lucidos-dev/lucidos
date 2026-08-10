import { describe, it, expect, afterEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import {
  resolveOrigin,
  executorExtras,
  resolveThreadLinkTitle,
  renderChannelSection,
  renderAuditSection,
  renderEngineExplainerSection,
  renderExecutorSection,
  renderInitiatorRow,
  renderOriginSection,
} from './MessageRoutePanel';
import { appsList, repositories } from '../../store/store';
import type { Exchange, StoredEvent, ThreadMeta } from '../../store/thread-events';

/** Wrap a single StoredEvent as an Exchange so each test can keep declaring
 *  the userEvent inline — `resolveOrigin` takes the full exchange (it walks
 *  steps for divider-starter ActionRequired events, see the divider cases
 *  below). */
function exch(userEvent: StoredEvent, steps: Exchange['steps'] = []): Exchange {
  return { userEvent, userSeq: 1, steps };
}

describe('resolveOrigin', () => {
  it('returns the explicit origin when present on MessageReceived', () => {
    const ev: StoredEvent = {
      type: 'MessageReceived',
      text: 'hi',
      mode: 'human',
      origin: { kind: 'workspace', workspace: 'myws', thread_id: 't1', event_id: 'e1' },
    };
    const o = resolveOrigin(exch(ev));
    expect(o).toEqual({ kind: 'workspace', workspace: 'myws', thread_id: 't1', event_id: 'e1' });
  });

  it('synthesizes a Device origin when only legacy device_id is set', () => {
    const ev: StoredEvent = {
      type: 'MessageReceived',
      text: 'hi',
      mode: 'human',
      device_id: 'dev-1',
      device: 'Chrome',
    };
    expect(resolveOrigin(exch(ev))).toEqual({ kind: 'device', device_id: 'dev-1', label: 'Chrome' });
  });

  it('synthesizes a ThreadLink (direction=parent) origin for agent-mode legacy events', () => {
    const ev: StoredEvent = {
      type: 'MessageReceived',
      text: 'hi',
      mode: 'agent',
      parent_thread_id: 'parent-id',
    };
    expect(resolveOrigin(exch(ev))).toEqual({
      kind: 'thread_link',
      thread_id: 'parent-id',
      spawning_event_id: undefined,
      mode: 'agent',
      direction: 'parent',
    });
  });

  it('returns undefined for non-MessageReceived events without an origin (panel branches separately)', () => {
    const ev: StoredEvent = { type: 'TriggerStarted', trigger_id: 't' };
    expect(resolveOrigin(exch(ev))).toBeUndefined();
  });

  it('extracts engine origin from ContinuationStarted events', () => {
    const ev: StoredEvent = {
      type: 'ContinuationStarted',
      branch: 'claude-code/x',
      origin: { kind: 'engine', reason: { kind: 'continuation_started' } },
    };
    expect(resolveOrigin(exch(ev))).toEqual({
      kind: 'engine',
      reason: { kind: 'continuation_started' },
    });
  });

  it('extracts engine origin from CodingAgentPromptSent events', () => {
    const ev: StoredEvent = {
      type: 'CodingAgentPromptSent',
      text: '/harden',
      origin: { kind: 'engine', reason: { kind: 'harden_retrigger' } },
    };
    expect(resolveOrigin(exch(ev))?.kind).toBe('engine');
  });

  it('extracts engine origin from TriggerStarted events when present', () => {
    const ev: StoredEvent = {
      type: 'TriggerStarted',
      trigger_id: 'abc',
      origin: { kind: 'engine', reason: { kind: 'scheduler', trigger_id: 'abc', trigger_name: 'nightly' } },
    };
    expect(resolveOrigin(exch(ev))).toEqual({
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
    expect(resolveOrigin(exch(ev))).toEqual({
      kind: 'engine',
      reason: { kind: 'stale_session' },
    });
  });

  // Regression: an auto-resume records neither `origin` nor `actor` (the device
  // that pressed Switch is on the teardown ResponseAborted, not here), so the
  // popover rendered a bare "Unknown" over a chip that already read "Lucidos
  // Engine". Only the engine can raise a ContinuationStarted nobody clicked, so
  // the missing field is a legacy gap, not an unknown actor.
  it('defaults ContinuationStarted with no origin and no actor to the engine (auto-resume / legacy DB row)', () => {
    const ev: StoredEvent = { type: 'ContinuationStarted', branch: 'x' };
    expect(resolveOrigin(exch(ev))).toEqual({
      kind: 'engine',
      reason: { kind: 'continuation_started' },
    });
  });

  it('defaults MissingHardeningDetected / MergeConflictDetected with no origin to their engine reason', () => {
    expect(resolveOrigin(exch({ type: 'MissingHardeningDetected' }))).toEqual({
      kind: 'engine',
      reason: { kind: 'missing_hardening' },
    });
    expect(resolveOrigin(exch({ type: 'MergeConflictDetected', files: ['a.rs'] }))).toEqual({
      kind: 'engine',
      reason: { kind: 'merge_conflict' },
    });
  });

  // The intrinsic default must not shadow a real actor: a user-clicked Continue
  // still has to read "You" on its device, not "Lucidos Engine".
  it('prefers a persisted origin over the intrinsic engine default', () => {
    const ev: StoredEvent = {
      type: 'MergeConflictDetected',
      origin: { kind: 'device', device_id: 'd1', label: 'Chrome on Mac' },
    };
    expect(resolveOrigin(exch(ev))).toEqual({ kind: 'device', device_id: 'd1', label: 'Chrome on Mac' });
  });

  // ResponseAborted is deliberately NOT in the intrinsic map: its own branch in
  // renderOriginSection keys on `origin === undefined` to render the System
  // attribution plus the typed AbortCause.
  it('leaves ResponseAborted unattributed so its System branch still fires', () => {
    expect(resolveOrigin(exch({ type: 'ResponseAborted', cause: 'engine_shutdown' }))).toBeUndefined();
  });

  // Regression: when the user clicks Continue on an interrupted CC thread the
  // backend stamps the device on `EventMeta.actor` (renders the chip as "You")
  // but until this fix forgot to mirror it onto the variant's `origin` field —
  // the popover then read `event.origin`, found nothing, and rendered
  // "Unknown" alongside the "You" chip.
  it('falls back to the actor on ContinuationStarted when origin is missing (user-clicked Continue)', () => {
    const ev: StoredEvent = {
      type: 'ContinuationStarted',
      branch: '',
      actor: { kind: 'device', device_id: 'dev-ios', label: 'iOS Safari PWA' },
    };
    expect(resolveOrigin(exch(ev))).toEqual({
      kind: 'device',
      device_id: 'dev-ios',
      label: 'iOS Safari PWA',
    });
  });

  it('returns undefined when device_id and parent_thread_id are both missing', () => {
    const ev: StoredEvent = { type: 'MessageReceived', text: 'hi', mode: 'human' };
    expect(resolveOrigin(exch(ev))).toBeUndefined();
  });

  it('surfaces the explicit actor on ChangeApplied', () => {
    const ev: StoredEvent = {
      type: 'ChangeApplied',
      change_id: 'c1',
      actor: { kind: 'device', device_id: 'd1', label: 'Chrome on Mac' },
    };
    expect(resolveOrigin(exch(ev))).toEqual({ kind: 'device', device_id: 'd1', label: 'Chrome on Mac' });
  });

  it('surfaces the actor on ChangeApplyFailed (so the failure has auditability)', () => {
    const ev: StoredEvent = {
      type: 'ChangeApplyFailed',
      change_id: 'c1',
      error: 'merge conflict',
      actor: { kind: 'api', user_agent: 'curl/8' },
    };
    expect(resolveOrigin(exch(ev))).toEqual({ kind: 'api', user_agent: 'curl/8' });
  });

  // Divider-starter ActionRequired events: the Origin is the device that
  // *answered* the question / *resolved* the permission — read from the
  // matching resolution step's actor.

  it('reads the answering device from UserQuestionAnswered.actor on a divider exchange', () => {
    const userEvent: StoredEvent = {
      type: 'UserQuestionAsked',
      tool_use_id: 'tu1',
      cc_session_id: 's1',
      question: 'Pick one',
      options: [{ id: 'a', label: 'A' }],
    };
    // Cast through unknown because UserQuestionAnswered's TS type doesn't yet
    // declare `actor` — Task 11 will land the Rust stamp + the regen.
    const answered = {
      type: 'UserQuestionAnswered',
      tool_use_id: 'tu1',
      answer: { kind: 'Selected', option_id: 'a' },
      actor: { kind: 'device', device_id: 'dev-ipad', label: 'iPad Safari' },
    } as unknown as StoredEvent;
    expect(resolveOrigin(exch(userEvent, [{ seq: 2, event: answered }]))).toEqual({
      kind: 'device', device_id: 'dev-ipad', label: 'iPad Safari',
    });
  });

  it('returns undefined for a pending UserQuestionAsked divider (no answer event yet)', () => {
    const userEvent: StoredEvent = {
      type: 'UserQuestionAsked',
      tool_use_id: 'tu1',
      cc_session_id: 's1',
      question: 'Pick one',
      options: [],
    };
    expect(resolveOrigin(exch(userEvent))).toBeUndefined();
  });

  it('reads the resolving device from CodingAgentPermissionResolved.actor on a divider exchange', () => {
    const userEvent: StoredEvent = {
      type: 'CodingAgentPermissionRequest',
      request_id: 'r1',
      tool_use_id: 'tu',
      tool_name: 'Bash',
      input: {},
      summary: 'ls',
    };
    const resolved = {
      type: 'CodingAgentPermissionResolved',
      request_id: 'r1',
      allowed: true,
      actor: { kind: 'device', device_id: 'dev-mac', label: 'Chrome on Mac' },
    } as unknown as StoredEvent;
    expect(resolveOrigin(exch(userEvent, [{ seq: 2, event: resolved }]))).toEqual({
      kind: 'device', device_id: 'dev-mac', label: 'Chrome on Mac',
    });
  });

  it('returns undefined for pending CodingAgentPermissionRequest divider (no resolution yet)', () => {
    const userEvent: StoredEvent = {
      type: 'CodingAgentPermissionRequest',
      request_id: 'r1',
      tool_use_id: 'tu',
      tool_name: 'Bash',
      input: {},
      summary: 'ls',
    };
    expect(resolveOrigin(exch(userEvent))).toBeUndefined();
  });

  it('returns undefined for CredentialRequested / McpConsentRequested (no answer event today)', () => {
    expect(resolveOrigin(exch({ type: 'CredentialRequested', provider: 'github' }))).toBeUndefined();
    expect(resolveOrigin(exch({ type: 'McpConsentRequested', tool: 'fs.read', args: {} }))).toBeUndefined();
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

  it('falls back to earlier SessionStarted for follow-up exchanges in the same Claude Code session', () => {
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
    // Two Claude Code sessions back-to-back: branch A then branch B. Follow-up exchange after B
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
    // The first exchange ran on branch A; later the user started a new Claude Code session on
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

  it('reads session id from CodingAgentSettingsChanged when SessionStarted carries an empty id (real Claude Code/Codex path)', () => {
    // The engine emits SessionStarted with session_id: "" at spawn; the real id
    // arrives from the agent's Init event on CodingAgentSettingsChanged — same
    // shape for both backends.
    const userEvent = stamp(0, { type: 'MessageReceived', text: 'go' });
    const sessionStarted = stamp(1, { type: 'SessionStarted', session_id: '', branch: 'claude-code/turn-1' });
    const init = stamp(2, { type: 'CodingAgentSettingsChanged', cc_session_id: 'real-sid', coding_agent: 'codex' });
    const exchange: Exchange = {
      userEvent,
      userSeq: 1,
      steps: [{ seq: 2, event: sessionStarted }, { seq: 3, event: init }],
    };
    const events = new Map<number, StoredEvent>([[1, userEvent], [2, sessionStarted], [3, init]]);
    const extras = executorExtras(exchange, events);
    expect(extras.branch).toBe('claude-code/turn-1');
    expect(extras.ccSessionId).toBe('real-sid');
  });

  it('carries the Init session id into a follow-up exchange in the same session', () => {
    // Turn 1: SessionStarted (empty id) + CodingAgentSettingsChanged (Init id).
    // Turn 2: no fresh Init — the panel must still report the session id.
    const t1User = stamp(0, { type: 'MessageReceived', text: 'first' });
    const sessionStarted = stamp(1, { type: 'SessionStarted', session_id: '', branch: 'claude-code/turn-1' });
    const init = stamp(2, { type: 'CodingAgentSettingsChanged', cc_session_id: 'real-sid' });
    const t2User = stamp(300, { type: 'MessageReceived', text: 'follow up' });

    const followUp: Exchange = { userEvent: t2User, userSeq: 10, steps: [] };
    const events = new Map<number, StoredEvent>([[1, t1User], [2, sessionStarted], [3, init], [10, t2User]]);
    const extras = executorExtras(followUp, events);
    expect(extras.ccSessionId).toBe('real-sid');
  });

  it('reads session id from CodingAgentIdled when no settings event carried it', () => {
    const userEvent = stamp(0, { type: 'MessageReceived', text: 'go' });
    const sessionStarted = stamp(1, { type: 'SessionStarted', session_id: '', branch: 'claude-code/x' });
    const idled = stamp(2, { type: 'CodingAgentIdled', has_changes: true, cc_session_id: 'idle-sid' });
    const exchange: Exchange = {
      userEvent,
      userSeq: 1,
      steps: [{ seq: 2, event: sessionStarted }, { seq: 3, event: idled }],
    };
    const events = new Map<number, StoredEvent>([[1, userEvent], [2, sessionStarted], [3, idled]]);
    const extras = executorExtras(exchange, events);
    expect(extras.ccSessionId).toBe('idle-sid');
  });

  it('a later settings change without a session id does not clear the established id', () => {
    const userEvent = stamp(0, { type: 'MessageReceived', text: 'go' });
    const sessionStarted = stamp(1, { type: 'SessionStarted', session_id: '', branch: 'claude-code/x' });
    const init = stamp(2, { type: 'CodingAgentSettingsChanged', cc_session_id: 'real-sid' });
    // User flips permission mode mid-session — this settings event carries no id.
    const permChange = stamp(3, { type: 'CodingAgentSettingsChanged', permission_mode: 'plan' });
    const exchange: Exchange = {
      userEvent,
      userSeq: 1,
      steps: [{ seq: 2, event: sessionStarted }, { seq: 3, event: init }, { seq: 4, event: permChange }],
    };
    const events = new Map<number, StoredEvent>([[1, userEvent], [2, sessionStarted], [3, init], [4, permChange]]);
    const extras = executorExtras(exchange, events);
    expect(extras.ccSessionId).toBe('real-sid');
  });

  it('reads branch from ContinuationStarted (engine restart resumes a Claude Code session)', () => {
    const recovered = stamp(0, { type: 'ContinuationStarted', branch: 'recovered-branch' });
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
    const thinking = stamp(2, { type: 'ThoughtStreamed', text: '...', context_tokens: 12345, trimmed: true });
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
  it('api origin with source_thread_id renders deep-link to spawning thread', () => {
    // The subprocess-origin path: `source_thread_id` set after the engine
    // recognised the request as coming from a Lucidos subprocess. The
    // popover must surface that link so a user can answer "which agent
    // did this".
    const node = renderChannelSection(
      { kind: 'api', user_agent: 'curl/8.7.1', mode: 'agent', source_thread_id: 'src-thread' },
      undefined,
      (tid) => tid === 'src-thread' ? 'Spawning thread title' : undefined,
    );
    const s = JSON.stringify(node);
    expect(s).toContain('curl/8.7.1');
    expect(s).toContain('Spawning thread title');
  });
  it('api origin with source_thread_id falls back to short id when no title resolver', () => {
    // When `getLiveTitle` returns undefined we still want a visible link —
    // a `thread <short>` placeholder so the popover is never blank.
    const node = renderChannelSection({
      kind: 'api',
      user_agent: 'curl/8.7.1',
      mode: 'agent',
      source_thread_id: '12345678-abcd-...',
    });
    const s = JSON.stringify(node);
    expect(s).toContain('curl/8.7.1');
    expect(s).toContain('thread 12345678');
  });
  it('workspace origin renders workspace name', () => {
    const node = renderChannelSection({
      kind: 'workspace', workspace: 'myws', mode: 'agent',
    });
    expect(JSON.stringify(node)).toContain('myws');
  });
  it('workspace origin with thread_id renders a thread link (short id when title unresolved)', () => {
    // No getLiveTitle and an empty current-workspace name (the test default) →
    // treated as local with no resolvable title → `thread <short>` placeholder
    // so the link is never blank.
    const node = renderChannelSection({
      kind: 'workspace',
      workspace: 'myws',
      thread_id: '12345678-aaaa-bbbb-cccc-dddddddddddd',
      mode: 'agent',
    });
    const s = JSON.stringify(node);
    expect(s).toContain('myws');
    expect(s).toContain('thread 12345678');
  });
  it('workspace origin renders the live thread name when the source thread is local', () => {
    // workspaceName defaults to '' in tests → the origin is treated as local →
    // the live `getLiveTitle` lookup wins over the short-id fallback.
    const node = renderChannelSection(
      { kind: 'workspace', workspace: 'dev', thread_id: 'tid', mode: 'agent' },
      undefined,
      (id) => (id === 'tid' ? 'Local thread name' : undefined),
    );
    expect(JSON.stringify(node)).toContain('Local thread name');
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
  it('workspace origin renders the event id but not the thread id (thread id is a channel link)', () => {
    const node = renderAuditSection({
      kind: 'workspace', workspace: 'p', thread_id: 'tid', event_id: 'eid', mode: 'agent',
    });
    const s = JSON.stringify(node);
    expect(s).toContain('eid');
    expect(s).not.toContain('tid');
  });
  it('workspace origin with only a thread id renders nothing (thread id moved to the channel)', () => {
    const node = renderAuditSection({
      kind: 'workspace', workspace: 'p', thread_id: 'tid', mode: 'agent',
    });
    expect(node).toBeNull();
  });
  it('parent_thread origin without spawning_event_id renders nothing', () => {
    const node = renderAuditSection({ kind: 'thread_link', thread_id: 't', mode: 'agent' });
    expect(node).toBeNull();
  });
  it('device/api/v1/engine origin renders nothing', () => {
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

describe('renderOriginSection', () => {
  const origin = (userEvent: StoredEvent): string =>
    JSON.stringify(renderOriginSection(exch(userEvent), undefined, () => undefined));

  // Regression: the auto-resume after a *Switch to new version* records the
  // pressing device on the teardown ResponseAborted, so the resume boundary
  // itself has no actor and no origin. The popover rendered a bare "Unknown"
  // under a chip that read "Lucidos Engine", even though the event carries a
  // typed `reason` that fully explains it.
  it('names the engine and the switch for an auto-resumed ContinuationStarted', () => {
    const s = origin({ type: 'ContinuationStarted', branch: '', reason: 'auto_resume_after_switch' });
    expect(s).toContain('Issued by');
    expect(s).toContain('Lucidos Engine');
    expect(s).toContain('Why this resumed');
    expect(s).toMatch(/Switch to new version/i);
    expect(s).not.toContain('Unknown');
  });

  it('keeps the engine attribution when a legacy resume recorded no reason', () => {
    const s = origin({ type: 'ContinuationStarted', branch: '' });
    expect(s).toContain('Lucidos Engine');
    // No event-level reason to be precise about, so it falls through to the
    // generic engine explanation rather than inventing a specific cause.
    expect(s).not.toContain('Why this resumed');
    expect(s).toContain('Why the engine acted');
    expect(s).not.toContain('Unknown');
  });

  // Regression: the reason-keyed explainer must LAYER OVER the engine one, not
  // replace it. A row that persisted `origin: engine{continuation_started}` but
  // no event-level reason used to render the engine explanation and has to keep
  // rendering it.
  it('falls back to the persisted engine reason when the event recorded no reason', () => {
    const s = origin({
      type: 'ContinuationStarted',
      branch: '',
      origin: { kind: 'engine', reason: { kind: 'continuation_started' } },
    });
    expect(s).toContain('Why the engine acted');
    expect(s).toMatch(/auto-resumed/i);
  });

  // The generic fallback is reached by chat and trigger resumes too, so it must
  // not name a coding agent.
  it('does not claim a Claude Code session in the generic resume explanation', () => {
    expect(origin({ type: 'ContinuationStarted', branch: '' })).not.toMatch(/Claude Code/);
  });

  // The device that clicked Continue still owns the turn: it must read as its
  // own device, never as the engine.
  it('attributes a user-clicked Continue to the clicking device, not the engine', () => {
    const s = origin({
      type: 'ContinuationStarted',
      branch: '',
      reason: 'user_clicked_continue',
      actor: { kind: 'device', device_id: 'd1', label: 'iOS Safari PWA' },
    });
    expect(s).toContain('iOS Safari PWA');
    expect(s).not.toContain('Lucidos Engine');
    expect(s).toMatch(/You clicked Continue/);
  });

  it('names the engine on a legacy MergeConflictDetected that predates the origin field', () => {
    const s = origin({ type: 'MergeConflictDetected', files: ['a.rs'] });
    expect(s).toContain('Lucidos Engine');
    expect(s).toContain('Why the engine acted');
    expect(s).not.toContain('Unknown');
  });

  // The System branch must survive the intrinsic-engine default above it.
  it('still renders the System attribution for an actor-less ResponseAborted', () => {
    const s = origin({ type: 'ResponseAborted', cause: 'engine_shutdown' });
    expect(s).toContain('System');
    expect(s).toContain('Why the response stopped');
    expect(s).not.toContain('Unknown');
  });

  it('still falls back to Unknown for a genuinely unattributed event', () => {
    expect(origin({ type: 'MessageReceived', text: 'hi', mode: 'human' })).toContain('Unknown');
  });
});

describe('renderInitiatorRow', () => {
  it('discloses Claude Code as the asker for UserQuestionAsked', () => {
    const node = renderInitiatorRow({
      type: 'UserQuestionAsked',
      tool_use_id: 'tu',
      cc_session_id: 's',
      question: 'q',
      options: [],
    });
    const s = JSON.stringify(node);
    expect(s).toContain('Asked by');
    expect(s).toContain('Claude Code');
  });

  it('discloses Claude Code (permission gate) for CodingAgentPermissionRequest', () => {
    const node = renderInitiatorRow({
      type: 'CodingAgentPermissionRequest',
      request_id: 'r1',
      tool_use_id: 'tu',
      tool_name: 'Edit',
      input: {},
      summary: 's',
    });
    const s = JSON.stringify(node);
    expect(s).toContain('Asked by');
    expect(s).toContain('Claude Code (permission gate)');
  });

  it('discloses Lucidos as the asker for CredentialRequested', () => {
    const node = renderInitiatorRow({ type: 'CredentialRequested', provider: 'github' });
    expect(JSON.stringify(node)).toContain('Lucidos (credential request)');
  });

  it('discloses Lucidos as the asker for McpConsentRequested', () => {
    const node = renderInitiatorRow({ type: 'McpConsentRequested', tool: 'fs.read', args: {} });
    expect(JSON.stringify(node)).toContain('Lucidos (tool consent)');
  });

  it('returns null for non-divider event types (their initiator is implied)', () => {
    expect(renderInitiatorRow({ type: 'MessageReceived', text: 'hi', mode: 'human' })).toBeNull();
    expect(renderInitiatorRow({ type: 'TriggerStarted', trigger_id: 't' })).toBeNull();
    expect(renderInitiatorRow({ type: 'ChangeApplied', change_id: 'c1' })).toBeNull();
  });
});

/** Drop the nodes preact renders as nothing, and flatten arrays, so what's left
 *  is exactly the elements the browser lays out. */
function renderedChildren(node: ComponentChildren): ComponentChildren[] {
  if (node === null || node === undefined || node === '' || typeof node === 'boolean') return [];
  if (Array.isArray(node)) return node.flatMap(renderedChildren);
  return [node];
}

/** Child count of every `.route-row` in the tree, in document order. */
function routeRowChildCounts(node: ComponentChildren, out: number[] = []): number[] {
  for (const child of renderedChildren(node)) {
    if (typeof child !== 'object') continue;
    const v = child as VNode<{ class?: string; children?: ComponentChildren }>;
    const children = v.props?.children;
    if (v.props?.class === 'route-row') out.push(renderedChildren(children).length);
    routeRowChildCounts(children, out);
  }
  return out;
}

/** A `.route-row` is `display: contents` (styles/panels/previews.css), so its
 *  children ARE the label and value cells of the section's shared two-column
 *  grid. That grid is what keeps every value starting at the same x, and it
 *  only holds while each row contributes exactly two items: a row that renders
 *  a third pushes every row below it one cell along and the panel goes ragged
 *  again. A multi-part value (app icon + name, user-agent + spawning-thread
 *  link) therefore has to bring its own `.route-value-group` box. */
describe('route rows contribute exactly two grid cells', () => {
  // The executor case seeds the repo + app registries so the render path reads
  // them instead of firing a fetch. Put them back, or a later test in this file
  // inherits a loaded registry it never asked for.
  afterEach(() => {
    repositories.value = { status: 'not-loaded' };
    appsList.value = { status: 'not-loaded' };
  });

  const expectTwoCellRows = (node: ComponentChildren, expectedRows: number): void => {
    const counts = routeRowChildCounts(node);
    expect(counts).toHaveLength(expectedRows);
    expect(counts.filter(n => n !== 2)).toEqual([]);
  };

  it('holds for every origin kind', () => {
    const section = (userEvent: StoredEvent): ComponentChildren =>
      renderOriginSection(exch(userEvent), 'Parent title', () => 'Live title');
    expectTwoCellRows(section({
      type: 'MessageReceived', text: 'hi', mode: 'human',
      origin: { kind: 'device', device_id: 'd1', label: 'Chrome on Mac' },
    }), 1);
    // The row that broke the grid before it was wrapped: an API origin from a
    // Lucidos subprocess renders a user-agent AND a deep-link to the spawning
    // thread, which used to be two siblings of the label.
    expectTwoCellRows(section({
      type: 'MessageReceived', text: 'hi', mode: 'human',
      origin: { kind: 'api', user_agent: 'curl/8.7.1', source_thread_id: 'src-thread' },
    }), 1);
    expectTwoCellRows(section({
      type: 'MessageReceived', text: 'hi', mode: 'human',
      origin: { kind: 'workspace', workspace: 'myws', thread_id: 't1', event_id: 'e1' },
    }), 2);
    expectTwoCellRows(section({
      type: 'MessageReceived', text: 'hi', mode: 'agent', parent_thread_id: 'p1',
    }), 1);
    expectTwoCellRows(section({ type: 'ContinuationStarted', branch: '' }), 1);
    expectTwoCellRows(section({ type: 'ResponseAborted', cause: 'engine_shutdown' }), 1);
    expectTwoCellRows(section({ type: 'ResponseCanceled', cause: 'user_stop' }), 1);
    expectTwoCellRows(section({
      type: 'CodingAgentPermissionRequest',
      request_id: 'r1', tool_use_id: 'tu', tool_name: 'Bash', input: {}, summary: 'ls',
    }), 1);
  });

  it('holds for a trigger origin (its rows are a fragment, not a wrapper div)', () => {
    const node = renderOriginSection(
      exch({
        type: 'TriggerStarted',
        trigger_id: 'tr1',
        trigger_name: 'nightly',
        invocation: { kind: 'Event', event_type: 'ChangeProposed', event_id: 'ev1' },
      }),
      undefined,
      () => undefined,
    );
    expectTwoCellRows(node, 2);
    // A wrapper element would take the whole grid row and leave the label and
    // value stacked inside it, so assert the rows really are section-level.
    expect(routeRowChildCounts(
      renderedChildren((node as VNode<{ children?: ComponentChildren }>).props?.children),
    )).toHaveLength(2);
  });

  it('holds for every executor row', () => {
    repositories.value = {
      status: 'loaded',
      data: [{ id: 'repo-1', name: 'Lucidos', path: '/tmp/lucidos' }],
    };
    appsList.value = {
      status: 'loaded',
      data: [{ id: 'habit-tracker', name: 'Habit Tracker', description: '', icon: '\u{1F9ED}' }],
    };
    const created = '2026-08-10T12:00:00.000Z';
    const userEvent: StoredEvent = { type: 'MessageReceived', text: 'go', created };
    const session: StoredEvent = {
      type: 'SessionStarted', session_id: '', branch: 'agent/turn-1', repo_id: 'repo-1',
      created: '2026-08-10T12:00:01.000Z',
    };
    const settings: StoredEvent = {
      type: 'CodingAgentSettingsChanged', cc_session_id: 'sid-1', permission_mode: 'plan',
      created: '2026-08-10T12:00:02.000Z',
    };
    const context: StoredEvent = {
      type: 'ContextCaptured', producer: 'claude_code', model: 'claude-opus-5[1m]',
      context_window: 1_000_000, estimated_total_tokens: 652_662, trimmed: true,
      created: '2026-08-10T12:00:03.000Z',
    };
    const exchange: Exchange = {
      userEvent,
      userSeq: 1,
      steps: [{ seq: 2, event: session }, { seq: 3, event: settings }, { seq: 4, event: context }],
    };
    const events = new Map<number, StoredEvent>([
      [1, userEvent], [2, session], [3, settings], [4, context],
    ]);
    const meta = {
      codingAgentKind: 'app',
      codingAgentFolder: 'data/apps/habit-tracker',
    } as unknown as ThreadMeta;

    // Model, Effort, Context, Permission, Repository, App, Branch, Session.
    expectTwoCellRows(
      renderExecutorSection(exchange, events, meta, 'claude-opus-5[1m]', 'xhigh'),
      8,
    );
  });
});
