import { describe, it, expect, afterEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import {
  executorExtras,
  resolveThreadLinkTitle,
  renderChannelSection,
  renderEngineExplainerSection,
  renderExecutorSection,
  renderOriginSection,
} from './MessageRoutePanel';
import { originPopoverHasContent, resolveOrigin } from './messageOrigin';
import { formatMessageTimestamp } from '../../utils/formatTime';
import { appsList, contextViewer, repositories } from '../../store/store';
import { devices } from '../../store/actions/devices';
import { pairedDevices } from '../../store/actions/pairedDevices';
import type { EngineReason, Exchange, StoredEvent, ThreadMeta } from '../../store/thread-events';

/** Wrap a single StoredEvent as an Exchange so each test can keep declaring
 *  the userEvent inline — `resolveOrigin` takes the full exchange (it walks
 *  steps for divider-starter ActionRequired events, see the divider cases
 *  below). */
function exch(userEvent: StoredEvent, steps: Exchange['steps'] = []): Exchange {
  return { userEvent, userSeq: 1, steps };
}


/** Seed the devices list a device origin's name is read from. */
function seedDevice(id: string, name: string): void {
  pairedDevices.value = { status: 'loaded', data: [] };
  devices.value = {
    status: 'loaded',
    data: [{ id, name, pairing_label: null, user_agent: null, push_enabled: false, last_seen_at: '', created_at: '' }],
  };
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
    };
    expect(resolveOrigin(exch(ev))).toEqual({ kind: 'device', device_id: 'dev-1' });
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
      origin: { kind: 'device', device_id: 'd1' },
    };
    expect(resolveOrigin(exch(ev))).toEqual({ kind: 'device', device_id: 'd1' });
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
      actor: { kind: 'device', device_id: 'dev-ios' },
    };
    expect(resolveOrigin(exch(ev))).toEqual({
      kind: 'device',
      device_id: 'dev-ios'
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
      actor: { kind: 'device', device_id: 'd1' },
    };
    expect(resolveOrigin(exch(ev))).toEqual({ kind: 'device', device_id: 'd1' });
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
      actor: { kind: 'device', device_id: 'dev-ipad' },
    } as unknown as StoredEvent;
    expect(resolveOrigin(exch(userEvent, [{ seq: 2, event: answered }]))).toEqual({
      kind: 'device', device_id: 'dev-ipad'
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
      actor: { kind: 'device', device_id: 'dev-mac' },
    } as unknown as StoredEvent;
    expect(resolveOrigin(exch(userEvent, [{ seq: 2, event: resolved }]))).toEqual({
      kind: 'device', device_id: 'dev-mac'
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

  it('returns undefined for McpConsentRequested (no answer event today)', () => {
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

  // A uuid is not a name. The id itself is in Technical details.
  it('falls back to "Untitled thread", never the id, when no source has a title', () => {
    const result = resolveThreadLinkTitle(
      { kind: 'thread_link', thread_id: parentId, mode: 'agent' },
      undefined,
      liveTitle(undefined),
    );
    expect(result).toBe('Untitled thread');
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
    // User switches model mid-session. This settings event carries no id.
    const modelChange = stamp(3, { type: 'CodingAgentSettingsChanged', model: 'opus' });
    const exchange: Exchange = {
      userEvent,
      userSeq: 1,
      steps: [{ seq: 2, event: sessionStarted }, { seq: 3, event: init }, { seq: 4, event: modelChange }],
    };
    const events = new Map<number, StoredEvent>([[1, userEvent], [2, sessionStarted], [3, init], [4, modelChange]]);
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

  it('still extracts context from the current exchange steps only', () => {
    const userEvent = stamp(0, { type: 'MessageReceived', text: 'go' });
    const thinking = stamp(2, { type: 'ThoughtStreamed', text: '...', context_tokens: 12345, trimmed: true });
    const exchange: Exchange = {
      userEvent,
      userSeq: 1,
      steps: [{ seq: 3, event: thinking }],
    };
    const events = new Map<number, StoredEvent>([[1, userEvent], [3, thinking]]);
    const extras = executorExtras(exchange, events);
    expect(extras.contextTokens).toBe(12345);
    expect(extras.contextTrimmed).toBe(true);
    // A legacy row has no snapshot behind it, so there is nothing to open.
    expect(extras.contextCapture).toBeUndefined();
  });

  it('carries the turn\u2019s last capture, so the Context row can open it', () => {
    const userEvent = stamp(0, { type: 'MessageReceived', text: 'go' });
    const first = stamp(1, {
      type: 'ContextCaptured', producer: 'claude_code', model: 'claude-sonnet-5',
      context_window: 1_000_000, sections: [], estimated_total_tokens: 438_000,
    });
    const last = {
      ...stamp(2, {
        type: 'ContextCaptured', producer: 'claude_code', model: 'claude-sonnet-5',
        context_window: 1_000_000, sections: [], estimated_total_tokens: 439_000,
      }),
      _eventId: 'evt-last',
    } as StoredEvent;
    const exchange: Exchange = { userEvent, userSeq: 1, steps: [{ seq: 2, event: first }, { seq: 3, event: last }] };
    const events = new Map<number, StoredEvent>([[1, userEvent], [2, first], [3, last]]);

    const extras = executorExtras(exchange, events);
    expect(extras.contextTokens).toBe(439_000);
    expect(extras.contextCapture?.estimated_total_tokens).toBe(439_000);
    expect(extras.contextCapture?.event_id).toBe('evt-last');
  });
});

describe('renderChannelSection', () => {
  it('device origin renders the name the devices list holds for its id', () => {
    seedDevice('d1', 'Chrome on Mac');
    const node = renderChannelSection({ kind: 'device', device_id: 'd1' });
    expect(JSON.stringify(node)).toContain('Chrome on Mac');
  });
  // The raw user-agent is machine words: it lives in Technical details.
  it('api origin names an outside app in words, not by its user-agent', () => {
    const s = JSON.stringify(renderChannelSection({ kind: 'api', user_agent: 'MyApp/1.0', mode: 'agent' }));
    expect(s).toContain('An app or script outside Lucidos');
    expect(s).not.toContain('MyApp/1.0');
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
    expect(s).toContain('A script run by the agent in');
    expect(s).toContain('Spawning thread title');
  });
  it('api origin with source_thread_id falls back to "Untitled thread" when no title resolves', () => {
    // The link is never blank, and never an id.
    const node = renderChannelSection({
      kind: 'api',
      user_agent: 'curl/8.7.1',
      mode: 'agent',
      source_thread_id: '12345678-abcd-...',
    });
    const s = JSON.stringify(node);
    expect(s).toContain('Untitled thread');
    expect(s).not.toContain('12345678');
  });
  it('workspace origin renders workspace name', () => {
    const node = renderChannelSection({
      kind: 'workspace', workspace: 'myws', mode: 'agent',
    });
    expect(JSON.stringify(node)).toContain('myws');
  });
  it('workspace origin with thread_id renders a thread link ("Untitled thread" when title unresolved)', () => {
    // No getLiveTitle and an empty current-workspace name (the test default):
    // treated as local with no resolvable title, so the link reads
    // "Untitled thread" rather than a blank or an id.
    const node = renderChannelSection({
      kind: 'workspace',
      workspace: 'myws',
      thread_id: '12345678-aaaa-bbbb-cccc-dddddddddddd',
      mode: 'agent',
    });
    const s = JSON.stringify(node);
    expect(s).toContain('myws');
    expect(s).toContain('Untitled thread');
    expect(s).not.toContain('12345678');
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

/** The section with its Technical details fold cut out, so a test can say a
 *  raw id appears there and nowhere else. */
function withoutTechnical(node: ComponentChildren): { body: string; technical: string } {
  let technical = '';
  const strip = (n: ComponentChildren): ComponentChildren => {
    if (Array.isArray(n)) return n.map(strip);
    if (!n || typeof n !== 'object') return n;
    const v = n as VNode<{ children?: ComponentChildren; 'data-role'?: string }>;
    if (v.props?.['data-role'] === 'route-technical') {
      technical += JSON.stringify(v);
      return null;
    }
    return { ...v, props: { ...v.props, children: strip(v.props?.children) } } as VNode;
  };
  return { body: JSON.stringify(strip(node)), technical };
}

describe('Technical details', () => {
  const section = (userEvent: StoredEvent) =>
    withoutTechnical(renderOriginSection(exch(userEvent), undefined, () => undefined));

  it('holds the raw event type, which no other line of the panel shows', () => {
    const { body, technical } = section({ type: 'PromptInjected', text: 'x', mode: 'human', origin: { kind: 'device', device_id: 'd1' } });
    expect(technical).toContain('Event type');
    expect(technical).toContain('PromptInjected');
    expect(body).not.toContain('PromptInjected');
  });

  it('holds the ids and client strings an origin carried, and only there', () => {
    const { body, technical } = section({
      type: 'MessageReceived', text: 'hi', mode: 'human',
      origin: { kind: 'workspace', workspace: 'myws', thread_id: 'tid-1', event_id: 'eid-1', user_agent: 'curl/8.7.1' },
    });
    for (const raw of ['tid-1', 'eid-1', 'curl/8.7.1']) {
      expect(technical).toContain(raw);
      expect(body).not.toContain(raw);
    }
  });

  it('holds a fired trigger\'s event id, and names its event in words above', () => {
    const { body, technical } = withoutTechnical(renderOriginSection(exch({
      type: 'TriggerStarted', trigger_id: 'trig-1', trigger_name: 'Nightly',
      invocation: { kind: 'Event', event_type: 'ChangeApplied', event_id: 'evt-9' },
    }), undefined, () => undefined));
    expect(body).toContain('change applied');
    expect(body).not.toContain('evt-9');
    expect(body).not.toContain('trig-1');
    expect(technical).toContain('evt-9');
  });
});

describe('renderEngineExplainerSection', () => {
  it('session_recovered renders explainer text', () => {
    const node = renderEngineExplainerSection({ kind: 'session_recovered' });
    expect(JSON.stringify(node)).toMatch(/picked it back up/i);
  });
  // A scheduled message used to show no reason at all, and its turn read
  // "Forwarded message". The trigger's name is the reason.
  it('scheduler names its trigger', () => {
    const node = renderEngineExplainerSection({ kind: 'scheduler', trigger_id: 't', trigger_name: 'Nightly' });
    expect(JSON.stringify(node)).toContain('“Nightly” ran on its schedule');
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
    expect(s).toMatch(/chose Switch on the new version/);
    expect(s).not.toContain('Unknown');
  });

  // The header of a read message shows the read time, so the sent time lives
  // here or nowhere.
  it('lists when a read message was sent and when it was read', () => {
    const message: StoredEvent = {
      type: 'MessageReceived', text: 'hi', mode: 'human',
      origin: { kind: 'device', device_id: 'd1' },
      created: '2026-09-24T06:13:01Z',
    };
    const read = JSON.stringify(renderOriginSection(
      exch(message), undefined, () => undefined, undefined, '2026-09-24T06:19:30Z',
    ));
    expect(read).toContain('"Sent"');
    expect(read).toContain('"Read"');

    const unread = origin(message);
    expect(unread).not.toContain('"Sent"');
    expect(unread).not.toContain('"Read"');
  });

  it('keeps the engine attribution when a legacy resume recorded no reason', () => {
    const s = origin({ type: 'ContinuationStarted', branch: '' });
    expect(s).toContain('Lucidos Engine');
    // No event-level reason to be precise about, so it falls through to the
    // generic engine explanation rather than inventing a specific cause.
    expect(s).not.toContain('Why this resumed');
    expect(s).toContain('Why Lucidos acted');
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
    expect(s).toContain('Why Lucidos acted');
    expect(s).toMatch(/picked it back up/i);
  });

  // The generic fallback is reached by chat and trigger resumes too, so it must
  // not name a coding agent.
  it('does not claim a Claude Code session in the generic resume explanation', () => {
    expect(origin({ type: 'ContinuationStarted', branch: '' })).not.toMatch(/Claude Code/);
  });

  // The device that clicked Continue still owns the turn: it must read as its
  // own device, never as the engine.
  it('attributes a user-clicked Continue to the clicking device, not the engine', () => {
    seedDevice('d1', 'iOS Safari PWA');
    const s = origin({
      type: 'ContinuationStarted',
      branch: '',
      reason: 'user_clicked_continue',
      actor: { kind: 'device', device_id: 'd1' },
    });
    expect(s).toContain('iOS Safari PWA');
    expect(s).not.toContain('Lucidos Engine');
    expect(s).toMatch(/You pressed Continue/);
  });

  it('names the engine on a legacy MergeConflictDetected that predates the origin field', () => {
    const s = origin({ type: 'MergeConflictDetected', files: ['a.rs'] });
    expect(s).toContain('Lucidos Engine');
    expect(s).toContain('Why Lucidos acted');
    expect(s).not.toContain('Unknown');
  });

  // The System branch must survive the intrinsic-engine default above it.
  it('still renders the System attribution for an actor-less ResponseAborted', () => {
    const s = origin({ type: 'ResponseAborted', cause: 'engine_shutdown' });
    expect(s).toContain('System');
    expect(s).toContain('Why the response stopped');
    expect(s).not.toContain('Unknown');
  });

  // The second reported card: a bare "Unknown". Nothing was recorded, so the
  // panel says that in words and claims no cause it cannot prove.
  it('says "Not recorded", never a bare Unknown, for a genuinely unattributed event', () => {
    const s = origin({ type: 'MessageReceived', text: 'hi', mode: 'human' });
    expect(s).toContain('Not recorded');
    expect(s).toContain('Lucidos did not record who started this turn.');
    expect(s).not.toContain('Unknown');
  });

  // An abort a person caused (a restart, a button that cleared a stuck reply)
  // still says why it stopped. It used to show only the device.
  it('explains why a device-attributed abort stopped', () => {
    seedDevice('d1', 'My MacBook');
    const s = origin({ type: 'ResponseAborted', cause: 'stale_settle', actor: { kind: 'device', device_id: 'd1' } });
    expect(s).toContain('Stopped from');
    expect(s).toContain('My MacBook');
    expect(s).toContain('Why the response stopped');
  });

  it('labels the device that answered a question "Answered on", not "Device"', () => {
    seedDevice('d1', 'My iPhone');
    const asked: StoredEvent = { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: 's', question: 'q', options: [] };
    const answered: StoredEvent = {
      type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'Selected', option_id: 'a' },
      actor: { kind: 'device', device_id: 'd1' },
    };
    const s = JSON.stringify(renderOriginSection(exch(asked, [{ seq: 2, event: answered }]), undefined, () => undefined));
    expect(s).toContain('Answered on');
    expect(s).toContain('My iPhone');
  });
});

describe('an event wait re-entry says where its words came from', () => {
  const origin = (userEvent: StoredEvent): string =>
    JSON.stringify(renderOriginSection(exch(userEvent), undefined, () => undefined));

  it('names the engine, what the agent waited for, and the story', () => {
    const s = origin({
      type: 'PromptInjected', text: 'A subscription you registered has timed out.', mode: 'agent',
      origin: { kind: 'engine', reason: { kind: 'event_wait', outcome: 'expired', watched: ['BenchSlotReleased'], wait_reason: 'the bench slot' } },
    });
    expect(s).toContain('Issued by');
    expect(s).toContain('Lucidos Engine');
    expect(s).toContain('Waited for');
    expect(s).toContain('bench slot released');
    expect(s).toContain('Why Lucidos acted');
    expect(s).toMatch(/Nothing happened before its deadline/);
    expect(s).not.toContain('Unknown');
  });

  // Rows from before the engine stamped an origin: every delivery and expiry
  // in a real workspace read "Unknown" here.
  it.each([
    { type: 'PromptInjected', text: 'A subscription you registered has timed out.\n\nTimed out.', mode: 'agent' },
    { type: 'PromptInjected', text: 'An event you subscribed to has arrived (you were waiting because: x).', mode: 'agent', delivered_event_id: 'e1' },
  ] as StoredEvent[])('an older re-entry anchor still names the engine and why', (userEvent) => {
    const s = origin(userEvent);
    expect(s).toContain('Lucidos Engine');
    expect(s).toContain('The agent asked Lucidos to tell it');
    expect(s).not.toContain('Unknown');
    expect(s).not.toContain('Not recorded');
  });
});

/** One fixture per `EngineReason` kind. A new kind is a `tsc` error until it
 *  gets a row here, so the "never Unknown" sweep below cannot skip it. */
const EVERY_ENGINE_REASON: { [K in EngineReason['kind']]: Extract<EngineReason, { kind: K }> } = {
  continuation_started: { kind: 'continuation_started' },
  session_recovered: { kind: 'session_recovered' },
  orphan_recovery: { kind: 'orphan_recovery' },
  scheduler: { kind: 'scheduler', trigger_id: 't1', trigger_name: 'nightly' },
  harden_retrigger: { kind: 'harden_retrigger' },
  event_wait: { kind: 'event_wait', outcome: 'delivered', watched: ['ChangeProposed'], wait_reason: 'a change' },
  stale_session: { kind: 'stale_session' },
  archived_branch_work: { kind: 'archived_branch_work' },
  merge_conflict: { kind: 'merge_conflict' },
  missing_hardening: { kind: 'missing_hardening' },
  missing_plan: { kind: 'missing_plan' },
  plugin_auto_update: {
    kind: 'plugin_auto_update', plugin_id: 'habit-tracker', marketplace_id: 'm1', marketplace_name: 'Example Market',
  },
  plugin_setup: {
    kind: 'plugin_setup', plugin_id: 'habit-tracker', plugin_name: '⏰ Habit Tracker', version: '0.1.4',
    occasion: { kind: 'fresh_install' },
  },
  plugin_upstream_proposal: {
    kind: 'plugin_upstream_proposal', plugin_id: 'habit-tracker', plugin_name: '⏰ Habit Tracker', version: '0.1.4',
    patch_path: 'artifacts/plugin-changes/habit-tracker/proposed-v0.1.4.patch',
  },
};

describe('the popover never reads Unknown for engine-authored work', () => {
  const origin = (userEvent: StoredEvent): string =>
    JSON.stringify(renderOriginSection(exch(userEvent), undefined, () => undefined));

  // Engine-seeded first messages, such as the plugin setup seed.
  it.each(Object.values(EVERY_ENGINE_REASON))('an engine-seeded message with reason $kind', (reason) => {
    const s = origin({ type: 'MessageReceived', text: 'seed', mode: 'engine', origin: { kind: 'engine', reason } });
    expect(s).toContain('Lucidos Engine');
    expect(s).not.toContain('Unknown');
  });

  it.each(Object.values(EVERY_ENGINE_REASON))('an engine prompt with reason $kind', (reason) => {
    const s = origin({ type: 'CodingAgentPromptSent', text: 'prompt', origin: { kind: 'engine', reason } });
    expect(s).not.toContain('Unknown');
  });

  it.each([
    { type: 'ContinuationStarted', branch: '' },
    { type: 'MissingHardeningDetected' },
    { type: 'MergeConflictDetected', files: ['a.rs'] },
  ] as StoredEvent[])('an actor-less $type', (userEvent) => {
    expect(origin(userEvent)).not.toContain('Unknown');
  });

  // Rows written before the engine stamped an origin: an old plugin setup seed
  // is agent-mode with no origin, no parent and no device. The chip already
  // says "Lucidos Engine", so the panel says so too, and admits the reason
  // was not recorded rather than inventing one.
  it.each(['agent', 'engine'] as const)('a legacy %s-mode message with nothing recorded', (mode) => {
    const s = origin({ type: 'MessageReceived', text: 'Set up X again.', mode });
    expect(s).toContain('Issued by');
    expect(s).toContain('Lucidos Engine');
    expect(s).toContain('Why Lucidos acted');
    expect(s).toMatch(/did not record why/);
    expect(s).not.toContain('Unknown');
  });
});

describe('plugin seeds name what the engine acted on', () => {
  const origin = (reason: EngineReason): string => JSON.stringify(renderOriginSection(
    exch({ type: 'MessageReceived', text: 'seed', mode: 'engine', origin: { kind: 'engine', reason } }),
    undefined,
    () => undefined,
  ));
  const setup = (occasion: Extract<EngineReason, { kind: 'plugin_setup' }>['occasion'], device?: string): EngineReason => ({
    kind: 'plugin_setup', plugin_id: 'habit-tracker', plugin_name: '⏰ Habit Tracker', version: '0.1.4',
    occasion, confirmed_on_device_id: device,
  });

  it('an update from a known version names both versions and the confirming device', () => {
    seedDevice('d1', 'My iPhone');
    const s = origin(setup({ kind: 'update', from_version: '0.1.3' }, 'd1'));
    expect(s).toContain('Plugin update');
    expect(s).toContain('⏰ Habit Tracker 0.1.3 → 0.1.4');
    expect(s).toContain('Confirmed on');
    expect(s).toContain('My iPhone');
    expect(s).toMatch(/You updated ⏰ Habit Tracker from 0\.1\.3 to 0\.1\.4/);
  });

  it('an update from an unrecorded version names only the new one', () => {
    const s = origin(setup({ kind: 'update' }));
    expect(s).toContain('Plugin update');
    expect(s).toContain('⏰ Habit Tracker 0.1.4');
    expect(s).not.toContain('→');
    expect(s).toMatch(/You updated ⏰ Habit Tracker to 0\.1\.4/);
    // No device recorded: no row claims one.
    expect(s).not.toContain('Confirmed on');
  });

  it('a fresh install reads as an install', () => {
    const s = origin(setup({ kind: 'fresh_install' }));
    expect(s).toContain('Plugin install');
    expect(s).toMatch(/You installed ⏰ Habit Tracker/);
  });

  it('a patch proposal names the plugin', () => {
    const s = origin(EVERY_ENGINE_REASON.plugin_upstream_proposal);
    expect(s).toContain('Plugin patch');
    expect(s).toContain('⏰ Habit Tracker 0.1.4');
    expect(s).toMatch(/propose it upstream/);
  });
});

// The chip on a question or permission card already names the agent that
// asked, so the popover must never only repeat it. It opens once someone
// answered, and says when and where.
describe('a question or permission card popover', () => {
  const question: StoredEvent = { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: '', question: 'q', options: [] };
  const answer = (kind: 'Selected' | 'Canceled', actor = true): StoredEvent => ({
    type: 'UserQuestionAnswered', tool_use_id: 'tu-1',
    answer: kind === 'Selected' ? { kind, option_id: 'a' } : { kind },
    created: '2026-10-06T17:22:00Z',
    ...(actor ? { actor: { kind: 'device', device_id: 'd1' } } : {}),
  } as StoredEvent);
  const permission: StoredEvent = {
    type: 'CodingAgentPermissionRequest', request_id: 'r1', tool_use_id: 'tu', tool_name: 'Edit', input: {}, summary: 's',
  };
  const verdict: StoredEvent = {
    type: 'CodingAgentPermissionResolved', request_id: 'r1', allowed: true,
    created: '2026-10-06T17:23:00Z', actor: { kind: 'device', device_id: 'd1' },
  } as StoredEvent;

  it('does not open while the question waits for an answer', () => {
    expect(originPopoverHasContent(exch(question))).toBe(false);
    expect(originPopoverHasContent(exch(permission))).toBe(false);
    expect(originPopoverHasContent(exch({ type: 'McpConsentRequested', tool: 'fs.read', args: {} }))).toBe(false);
  });

  it('does not open for a question nobody answered, even if it was settled', () => {
    expect(originPopoverHasContent(exch(question, [{ seq: 2, event: answer('Canceled') }]))).toBe(false);
    expect(originPopoverHasContent(exch(question, [{ seq: 2, event: answer('Selected', false) }]))).toBe(false);
  });

  it('opens once answered, and names when and on which device', () => {
    seedDevice('d1', 'My iPhone');
    const answered = exch(question, [{ seq: 2, event: answer('Selected') }]);
    expect(originPopoverHasContent(answered)).toBe(true);
    const s = JSON.stringify(renderOriginSection(answered, undefined, () => undefined));
    expect(s).toContain('"Answered"');
    expect(s).toContain(formatMessageTimestamp('2026-10-06T17:22:00Z'));
    expect(s).toContain('Answered on');
    expect(s).toContain('My iPhone');
  });

  it('names when a permission was decided', () => {
    seedDevice('d1', 'My iPhone');
    const decided = exch(permission, [{ seq: 2, event: verdict }]);
    expect(originPopoverHasContent(decided)).toBe(true);
    const s = JSON.stringify(renderOriginSection(decided, undefined, () => undefined));
    expect(s).toContain('"Decided"');
    expect(s).toContain(formatMessageTimestamp('2026-10-06T17:23:00Z'));
    expect(s).toContain('Decided on');
  });

  it('never repeats the asker the chip already names', () => {
    seedDevice('d1', 'My iPhone');
    const s = JSON.stringify(renderOriginSection(exch(question, [{ seq: 2, event: answer('Selected') }]), undefined, () => undefined));
    expect(s).not.toContain('Asked by');
    expect(s).not.toContain('Lucidos Agent');
  });

  it('leaves every other turn\'s popover alone', () => {
    expect(originPopoverHasContent(exch({ type: 'MessageReceived', text: 'hi', mode: 'human' }))).toBe(true);
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
      origin: { kind: 'device', device_id: 'd1' },
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
    expectTwoCellRows(renderOriginSection(exch(
      {
        type: 'CodingAgentPermissionRequest',
        request_id: 'r1', tool_use_id: 'tu', tool_name: 'Bash', input: {}, summary: 'ls',
      },
      [{
        seq: 2,
        event: {
          type: 'CodingAgentPermissionResolved', request_id: 'r1', allowed: true,
          created: '2026-10-06T17:23:00Z', actor: { kind: 'device', device_id: 'd1' },
        } as StoredEvent,
      }],
    ), undefined, () => undefined), 2);
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
      data: [{ id: 'habit-tracker', name: 'Habit Tracker', description: '', reveal: 'on-load', kind: 'app', reusable: false, icon: '\u{1F9ED}' }],
    };
    const created = '2026-08-10T12:00:00.000Z';
    const userEvent: StoredEvent = { type: 'MessageReceived', text: 'go', created };
    const session: StoredEvent = {
      type: 'SessionStarted', session_id: '', branch: 'agent/turn-1', repo_id: 'repo-1',
      created: '2026-08-10T12:00:01.000Z',
    };
    const settings: StoredEvent = {
      type: 'CodingAgentSettingsChanged', cc_session_id: 'sid-1',
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

    // Model, Effort, Context, Repository, App, Branch. The session id is in
    // Technical details, whose fold body brings its own grid.
    expectTwoCellRows(
      renderExecutorSection(exchange, events, meta, 'claude-opus-5[1m]', 'xhigh'),
      6,
    );
  });
});

describe('the Context row opens what the turn\u2019s last call was sent', () => {
  afterEach(() => { contextViewer.value = null; });

  const created = '2026-08-10T12:00:00.000Z';
  const userEvent: StoredEvent = { type: 'MessageReceived', text: 'go', created };
  const contextCell = (step: StoredEvent): VNode<{ children?: ComponentChildren }> => {
    const exchange: Exchange = { userEvent, userSeq: 1, steps: [{ seq: 2, event: step }] };
    const events = new Map<number, StoredEvent>([[1, userEvent], [2, step]]);
    const cells = routeRowCells(renderExecutorSection(exchange, events, {} as ThreadMeta), 'Context');
    expect(cells, 'no Context row rendered').not.toBeNull();
    return cells![1] as VNode<{ children?: ComponentChildren }>;
  };
  const buttonIn = (cell: VNode<{ children?: ComponentChildren }>) =>
    renderedChildren(cell.props?.children).find(
      (c): c is VNode<{ class?: string; onClick?: () => void }> =>
        typeof c === 'object' && (c as VNode).type === 'button',
    );

  it('is a link that opens the context viewer on the last capture', () => {
    const cell = contextCell({
      type: 'ContextCaptured', producer: 'claude_code', model: 'claude-sonnet-5',
      context_window: 1_000_000, sections: [], estimated_total_tokens: 356_429, created,
    } as StoredEvent);
    const link = buttonIn(cell);
    expect(link?.props.class).toBe('accent-link');

    link!.props.onClick!();
    expect(contextViewer.value?.snapshot.estimated_total_tokens).toBe(356_429);
  });

  it('stays plain text on a legacy row with no snapshot', () => {
    const cell = contextCell({
      type: 'ThoughtStreamed', text: '...', context_tokens: 12_345, created,
    } as StoredEvent);
    expect(buttonIn(cell)).toBeUndefined();
  });
});

/** The two cells of the `.route-row` whose label reads `label`. */
function routeRowCells(node: ComponentChildren, label: string): ComponentChildren[] | null {
  for (const child of renderedChildren(node)) {
    if (typeof child !== 'object') continue;
    const v = child as VNode<{ class?: string; children?: ComponentChildren }>;
    const cells = renderedChildren(v.props?.children);
    const heading = cells[0] as VNode<{ children?: ComponentChildren }> | undefined;
    if (v.props?.class === 'route-row' && renderedChildren(heading?.props?.children)[0] === label) {
      return cells;
    }
    const found = routeRowCells(v.props?.children, label);
    if (found) return found;
  }
  return null;
}

/** The App row's name span, the second child of its `.route-value-group`. */
function appNameSpan(node: ComponentChildren): VNode<{ class?: string }> {
  const cells = routeRowCells(node, 'App');
  expect(cells, 'no App row rendered').not.toBeNull();
  const group = cells![1] as VNode<{ children?: ComponentChildren }>;
  return renderedChildren(group.props?.children)[1] as VNode<{ class?: string }>;
}

/** The App row is a value like any other. Its name takes the panel's secondary
 *  text by inheritance rather than a class of its own. It used to carry
 *  `--text-primary` at weight 500, which read as the one emphasized value in a
 *  panel of plain ones. A class here is how that comes back. */
describe('the App row names its app in ordinary value text', () => {
  const meta = {
    codingAgentKind: 'app',
    codingAgentFolder: 'data/apps/habit-tracker',
  } as unknown as ThreadMeta;
  const session: StoredEvent = { type: 'SessionStarted', session_id: '', branch: 'agent/turn-1' };
  const exchange: Exchange = {
    userEvent: { type: 'MessageReceived', text: 'go' },
    userSeq: 1,
    steps: [{ seq: 2, event: session }],
  };
  const events = new Map<number, StoredEvent>([[1, exchange.userEvent], [2, session]]);
  const render = (): ComponentChildren => renderExecutorSection(exchange, events, meta);

  afterEach(() => {
    appsList.value = { status: 'not-loaded' };
  });

  it('gives the name no class of its own', () => {
    appsList.value = {
      status: 'loaded',
      data: [{ id: 'habit-tracker', name: 'Habit Tracker', description: '', reveal: 'on-load', kind: 'app', reusable: false, icon: '\u{1F9ED}' }],
    };
    expect(appNameSpan(render()).props.class).toBeUndefined();
  });

  it('marks the name as an error when the app list failed to load', () => {
    appsList.value = { status: 'failed', error: 'boom' };
    expect(appNameSpan(render()).props.class).toBe('error-text');
  });
});
