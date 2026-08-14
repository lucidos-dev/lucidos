import { describe, it, expect } from 'vitest';
import { describeInitiator } from '../ChatExchange';
import { ENGINE_LABEL, LUCIDOS_AGENT_LABEL, SYSTEM_LABEL, type Exchange } from '../../../store/thread-events';
import type { ComponentChildren, VNode } from 'preact';

function exchangeWith(userEvent: Exchange['userEvent']): Exchange {
  return { userEvent, userSeq: 0, steps: [] };
}

interface AnyVNode extends VNode<{ children?: ComponentChildren; class?: string; [k: string]: unknown }> {}

// Every initiator panel follows the same shape: `label` is WHO performed the
// action, `summary` is WHAT was done. The popover surfaces extra origin info,
// but the panel itself reads as "[icon] Lucidos Engine — Hardening required".
describe('describeInitiator — label is WHO, summary is WHAT', () => {
  it('human-mode MessageReceived: label "You", no summary, user variant', () => {
    const ex = exchangeWith({
      type: 'MessageReceived',
      text: 'hello',
      mode: 'human',
      channel: 'chat',
    });
    const desc = describeInitiator(ex, '<p>hello</p>', [], 'tid');
    expect(desc.label).toBe('You');
    expect(desc.summary).toBeUndefined();
    expect(desc.variant).toBe('user');
  });

  it('agent-mode MessageReceived (parent_thread origin): label is "Lucidos Agent", summary "Forwarded message", lucidos variant', () => {
    // Parent thread's LLM kicked off this child via run_thread — the Lucidos
    // agent (not the engine itself) is the initiator, so the WHO label must be
    // "Lucidos Agent" and the panel accent must be the agent's violet.
    // The engine label/variant stays for engine-internal triggers
    // (ContinuationStarted, MissingHardeningDetected, scheduler with no human, …).
    const ex = exchangeWith({
      type: 'MessageReceived',
      text: '[Child thread completed] ...',
      mode: 'agent',
      channel: 'chat',
      origin: { kind: 'thread_link', thread_id: 'parent-1' },
    });
    const desc = describeInitiator(ex, '<p>...</p>', [], 'tid');
    expect(desc.label).toBe(LUCIDOS_AGENT_LABEL);
    expect(desc.summary).toBe('Forwarded message');
    expect(desc.variant).toBe('lucidos');
  });

  it('API-originated MessageReceived (human mode): chip is "API caller", summary "API message"', () => {
    // "You" is reserved for `kind: device`. An anonymous HTTP caller never
    // impersonates the user in the timeline — the chip renders "API caller"
    // and the popover discloses the user-agent for forensics.
    const ex = exchangeWith({
      type: 'MessageReceived',
      text: 'curl request',
      mode: 'human',
      channel: 'chat',
      origin: { kind: 'api', user_agent: 'curl/8.7.1' },
    });
    const desc = describeInitiator(ex, '<p>curl request</p>', [], 'tid');
    expect(desc.label).toBe('API caller');
    expect(desc.summary).toBe('API message');
    expect(desc.variant).toBe('system');
  });

  it('ContinuationStarted (no actor): label falls back to engine, summary "Resumed after engine restart"', () => {
    // Legacy DB rows / auto-resume case carry no actor; chip falls back to the
    // Lucidos-mark Engine chip.
    const ex = exchangeWith({ type: 'ContinuationStarted' });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBe('Resumed after engine restart');
    expect(desc.variant).toBe('system');
  });

  it('ContinuationStarted (device actor): iconless action label "Continued the response" (ResponseCanceled style)', () => {
    // You clicked Continue — rendered like the cancel boundary: no icon, the
    // action IS the label, no separate summary; attribution is in the popover.
    const ex = exchangeWith({
      type: 'ContinuationStarted',
      actor: { kind: 'device', device_id: 'd-1', label: 'My Mac' },
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.icon).toBeNull();
    expect(desc.label).toBe('Continued the response');
    expect(desc.summary).toBeUndefined();
    expect(desc.variant).toBe('system');
  });

  it('ContinuationStarted (auto_recovery_after_hang): NOT labeled an engine restart', () => {
    // A hung subprocess OR a stray signal-kill (e.g. another workspace's
    // `cargo check` build-lock kill landing on this CC subprocess) auto-resumes
    // with reason=auto_recovery_after_hang — nothing restarted, so it must not
    // claim "Resumed after engine restart" (the wording that made a user think
    // restarting an unrelated workspace had restarted theirs). The reason wins
    // over the actor.
    const ex = exchangeWith({
      type: 'ContinuationStarted',
      reason: 'auto_recovery_after_hang',
      actor: { kind: 'system' },
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.summary).toBe('Resumed after the session stopped responding');
    expect(desc.summary).not.toBe('Resumed after engine restart');
  });

  it('ResponseAborted (system actor): chip is "System", summary "Response interrupted"', () => {
    // The host system killed the underlying process (engine shutdown,
    // safety-net catch, OS signal). Engine just marked it on recovery.
    const ex = exchangeWith({
      type: 'ResponseAborted',
      actor: { kind: 'system' },
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe(SYSTEM_LABEL);
    expect(desc.summary).toBe('Response interrupted');
    expect(desc.variant).toBe('system');
  });

  it('ResponseAborted (legacy engine actor): falls back to "Lucidos Engine"', () => {
    // Historical DB rows pre-System variant carry `Engine{OrphanRecovery}`.
    // The frontend keeps the old label — we don't migrate old rows.
    const ex = exchangeWith({
      type: 'ResponseAborted',
      actor: { kind: 'engine', reason: { kind: 'orphan_recovery' } },
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBe('Response interrupted');
    expect(desc.variant).toBe('system');
  });

  it('ResponseAborted (switch teardown): iconless action label "Paused by restart" (ResponseCanceled style)', () => {
    // /api/v1/restart → abort_in_flight_for_restart pre-emits `engine_shutdown`
    // with the device actor that hit Restart. BOTH halves are the fingerprint
    // (`isSwitchTeardownAbort`), so the fixture carries both. Rendered like the
    // cancel boundary: no icon, the action ("Paused by restart") IS the label, no
    // separate summary. The wording matches the `paused` thread status the same
    // abort leaves.
    const ex = exchangeWith({
      type: 'ResponseAborted',
      cause: 'engine_shutdown',
      actor: { kind: 'device', device_id: 'd-1', label: 'iOS Safari PWA' },
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.icon).toBeNull();
    expect(desc.label).toBe('Paused by restart');
    expect(desc.summary).toBeUndefined();
  });

  it('ResponseAborted (device actor, no cause): reads as an interruption, not a pause', () => {
    // A device actor ALONE is not the switch fingerprint, so this legacy row
    // (persisted before `cause` existed) reads "Response interrupted". The
    // wording on those old transcript rows is a deliberate cost of keeping the
    // frontend fingerprint identical to the backend's: `SWITCH_TEARDOWN_ABORT_SQL`
    // requires `cause = 'engine_shutdown'` too, so a legacy row is not a switch on
    // either side. Making only the frontend lenient is what would drift.
    //
    // Still the iconless action style: the branch keys on the device actor, and
    // whatever the cause, a device-attributed abort IS something the user did.
    const ex = exchangeWith({
      type: 'ResponseAborted',
      actor: { kind: 'device', device_id: 'd-1', label: 'iOS Safari PWA' },
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.icon).toBeNull();
    expect(desc.label).toBe('Response interrupted');
  });

  /** The user's **Stop waiting**, in the same iconless boundary style as the
   *  cancel below it and for the same reason: a person did something to this
   *  thread, and the action IS the header. The header names what was stopped,
   *  because once the clock indicator drops the wait this line is the only
   *  place the subscription is named at all. */
  it('EventWaitCanceled (user stop): iconless action label naming what was stopped', () => {
    const ex = exchangeWith({
      type: 'EventWaitCanceled',
      wait_id: 'w1',
      cause: 'user_stop',
      reason: "tonight's E2E suite to pass",
      actor: { kind: 'device', device_id: 'd-1', label: 'iOS Safari PWA' },
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.icon).toBeNull();
    expect(desc.label).toBe("Stopped waiting: tonight's E2E suite to pass");
    expect(desc.summary).toBeUndefined();
    // The chip stays clickable: the popover is where the device that pressed
    // the button is disclosed, off this event's own `actor`.
    expect(desc.actorClickable).not.toBe(false);
  });

  /** A pre-2026-08-07 row carries no reason, so the line says the one thing it
   *  knows rather than trailing an empty colon. */
  it('EventWaitCanceled (user stop, legacy row): names no subscription it cannot name', () => {
    const ex = exchangeWith({ type: 'EventWaitCanceled', wait_id: 'w1', cause: 'user_stop' });
    expect(describeInitiator(ex, '', [], 'tid').label).toBe('Stopped waiting for an event');
  });

  it('ResponseCanceled: no chip — "Response canceled" IS the header label, no summary, no icon', () => {
    // The cancel boundary drops the actor chip: the label is the header text and
    // clicking it opens the Initiator info popover. icon is null so ActorChipBody
    // skips the glyph span. Holds whether or not an actor was plumbed through.
    const ex = exchangeWith({ type: 'ResponseCanceled' });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe('Response canceled');
    expect(desc.summary).toBeUndefined();
    expect(desc.icon).toBeNull();
    expect(desc.variant).toBe('system');
  });

  it('ResponseCanceled with a device actor: still chromeless "Response canceled" (actor surfaces in the popover, not the chip)', () => {
    const ex = exchangeWith({
      type: 'ResponseCanceled',
      actor: { kind: 'device', device_id: 'd1', label: 'iPhone' },
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe('Response canceled');
    expect(desc.icon).toBeNull();
    expect(desc.summary).toBeUndefined();
  });

  it('MissingHardeningDetected: label is engine, summary "Hardening required"', () => {
    const ex = exchangeWith({ type: 'MissingHardeningDetected' });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBe('Hardening required');
    expect(desc.variant).toBe('system');
  });

  it('MergeConflictDetected: label is engine, summary "Merging changes from main"', () => {
    const ex = exchangeWith({ type: 'MergeConflictDetected', files: ['a.rs', 'b.rs'] });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBe('Merging changes from main');
    expect(desc.variant).toBe('system');
  });

  it('UserPromptInjected (no origin): label is engine, summary "Auto-prompt sent"', () => {
    const ex = exchangeWith({ type: 'UserPromptInjected', text: 'do X' });
    const desc = describeInitiator(ex, '<p>do X</p>', [], 'tid');
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBe('Auto-prompt sent');
    expect(desc.variant).toBe('system');
  });

  /** An event delivery is the ONE injection whose text is not its content:
   *  the prose is the model's prompt and carries the matched payload as
   *  pretty-printed JSON, which is a screen of raw JSON in a transcript. When
   *  the resolved delivery is in hand, the event row in the body names the event
   *  instead.
   *
   *  **No panel summary line**, because that row already reads "Event arrived:
   *  <event>". Carrying one printed the same words twice, once as plain prose in
   *  the header and once in the card underneath (reported 2026-08-10). Same as
   *  `TriggerStarted` and `ChildThreadCompleted` above, whose rows own their
   *  prefixes too. An injection with no resolved delivery is NOT this case and
   *  keeps its prose summary, covered by the test below. */
  it('UserPromptInjected (event delivery): the row names the event, the header stays quiet', () => {
    const ex = exchangeWith({
      type: 'UserPromptInjected',
      text: 'An event you subscribed to has arrived …\n\nCodingAgentIdled:\n{ … }',
      delivered_event_id: 'evt-1',
    });
    const desc = describeInitiator(
      ex, '<p>raw json</p>', [], 'tid', false, false, 'claude-code', undefined, undefined,
      { eventType: 'CodingAgentIdled', eventId: 'evt-src-1', payloadJson: '{\n  "has_changes": true\n}' },
    );
    expect(desc.summary).toBeUndefined();
    const body = desc.details as { type?: { name?: string }; props?: Record<string, unknown> };
    expect(body?.type?.name).toBe('EventDeliveryBody');
    // Each field lands on its own prop. They were three trailing
    // `string | undefined` positionals until 2026-08-10, and adding one in the
    // middle re-bound the payload to it with no type error and no failing
    // assertion; one object argument makes that unrepresentable.
    expect(body?.props).toMatchObject({
      eventType: 'CodingAgentIdled',
      eventId: 'evt-src-1',
      payloadJson: '{\n  "has_changes": true\n}',
    });
  });

  /** The link can dangle: the delivery sits in an earlier exchange, so a long
   *  thread can load the anchor without it. Falling back to the prose is the
   *  honest thing to show when the structured half is not in hand. */
  it('UserPromptInjected (delivery not loaded): falls back to the prose body', () => {
    const ex = exchangeWith({
      type: 'UserPromptInjected',
      text: 'An event you subscribed to has arrived …',
      delivered_event_id: 'evt-gone',
    });
    const desc = describeInitiator(ex, '<p>raw json</p>', [], 'tid');
    expect(desc.summary).toBe('Auto-prompt sent');
    expect(desc.details).toBeDefined();
  });

  // Regression: a child→parent callback emits UserPromptInjected with
  // origin = ThreadLink { direction: 'child', mode: 'agent' }. The chip must
  // render "Lucidos Agent", not "Lucidos Engine".
  it('UserPromptInjected (child callback): label is Lucidos Agent', () => {
    const ex = exchangeWith({
      type: 'UserPromptInjected',
      text: '[Child thread completed] ...',
      mode: 'agent',
      origin: { kind: 'thread_link', thread_id: 'child-1', mode: 'agent', direction: 'child' },
    });
    const desc = describeInitiator(ex, '<p>...</p>', [], 'tid');
    expect(desc.label).toBe('Lucidos Agent');
    expect(desc.variant).toBe('lucidos');
  });

  // Change lifecycle events: label is the actor (who did it), summary is the action.
  it('ChangeApplied (engine actor): label is engine, summary "Change applied"', () => {
    const ex = exchangeWith({
      type: 'ChangeApplied',
      change_id: 'c1',
      actor: { kind: 'engine', reason: { kind: 'session_recovered' } },
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBe('Change applied');
    expect(desc.accent).toBe('change-applied');
  });

  it('ChangeApplied (device actor): label "You", summary "Change applied"', () => {
    const ex = exchangeWith({
      type: 'ChangeApplied',
      change_id: 'c1',
      actor: { kind: 'device', device_id: 'd1', label: 'iPhone' },
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe('You');
    expect(desc.summary).toBe('Change applied');
  });

  it('ChangeApplied (no actor): defaults to engine label', () => {
    const ex = exchangeWith({ type: 'ChangeApplied', change_id: 'c1' });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBe('Change applied');
  });

  it('ChangeDiscarded (device): label "You", summary "Change discarded"', () => {
    const ex = exchangeWith({
      type: 'ChangeDiscarded',
      change_id: 'c1',
      actor: { kind: 'device', device_id: 'd1', label: 'Mac' },
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe('You');
    expect(desc.summary).toBe('Change discarded');
    expect(desc.accent).toBe('change-discarded');
  });

  it('ChangeReverted (device): label "You", summary "Change reverted"', () => {
    const ex = exchangeWith({
      type: 'ChangeReverted',
      change_id: 'c1',
      actor: { kind: 'device', device_id: 'd1', label: 'Mac' },
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe('You');
    expect(desc.summary).toBe('Change reverted');
    expect(desc.accent).toBe('change-reverted');
  });

  it('ChangeApplyFailed: label is engine, summary "Change failed"', () => {
    const ex = exchangeWith({ type: 'ChangeApplyFailed', change_id: 'c1', error: 'boom' });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBe('Change failed');
    expect(desc.accent).toBe('change-failed');
  });

  /** No panel summary line: the event row in the body reads "Trigger fired:
   *  morning-summary", so a header saying "Trigger fired" above it would state
   *  the same thing twice. Same shape as `ChildThreadCompleted` below, whose row
   *  has owned its own prefix all along. The popover's Origin row is unaffected:
   *  it renders the trigger through `renderTriggerOrigin`, not through this. */
  it('TriggerStarted: label is engine, no panel summary line (the event row owns the prefix)', () => {
    const ex = exchangeWith({
      type: 'TriggerStarted',
      trigger_id: 't1',
      trigger_name: 'morning-summary',
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBeUndefined();
    expect(desc.variant).toBe('trigger');
    expect((desc.details as { type?: { name?: string } })?.type?.name).toBe('TriggerFiredBody');
  });

  it('ChildThreadCompleted: label is "Lucidos Engine" (engine fan-in raises it), system variant, no panel summary line (card owns the prefix)', () => {
    const ex = exchangeWith({
      type: 'ChildThreadCompleted',
      child_thread_id: 'child-1',
      child_thread_title: 'Refactor foo',
      status: 'success',
      summary: 'cleaned up.',
      pending_change_ids: [],
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBeUndefined();
    expect(desc.variant).toBe('system');
  });

  it('ChildThreadCompleted: details embed the ChildCompletionRow with child_thread_id, title, status, and summary', () => {
    const ex = exchangeWith({
      type: 'ChildThreadCompleted',
      child_thread_id: 'child-1',
      child_thread_title: 'Refactor foo',
      status: 'success',
      summary: 'done.',
      pending_change_ids: ['cid-1'],
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    const node = desc.details as AnyVNode | undefined;
    expect(node).toBeDefined();
    expect(typeof node!.type).toBe('function');
    expect((node!.type as { name?: string }).name).toBe('ChildCompletionRow');
    const props = node!.props as Record<string, unknown>;
    expect(props.childThreadId).toBe('child-1');
    expect(props.childThreadTitle).toBe('Refactor foo');
    expect(props.status).toBe('success');
    expect(props.summary).toBe('done.');
  });
});

// User-driven control turns render in the "Response canceled" style: no icon,
// the action AS the label, attribution in the popover. Only the device-owner
// case is affected — engine/system-driven variants keep their chip.
describe('describeInitiator — user control turns render iconless (ResponseCanceled style)', () => {
  it('UserPromptInjected (device origin): iconless label "Auto-prompt sent", injected body kept', () => {
    const ex = exchangeWith({
      type: 'UserPromptInjected',
      text: 'do X',
      origin: { kind: 'device', device_id: 'd-1', label: 'Mac' },
    });
    const desc = describeInitiator(ex, '<p>do X</p>', [], 'tid');
    expect(desc.icon).toBeNull();
    expect(desc.label).toBe('Auto-prompt sent');
    expect(desc.summary).toBeUndefined();
    expect(desc.details).toBeDefined();
  });

  it('CredentialRequested / McpConsentRequested: iconless label = the request summary', () => {
    const cred = describeInitiator(exchangeWith({ type: 'CredentialRequested', provider: 'github' }), '', [], 'tid');
    expect(cred.icon).toBeNull();
    expect(cred.label).toBe('Credentials requested: github');
    expect(cred.variant).toBe('system');

    const mcp = describeInitiator(exchangeWith({ type: 'McpConsentRequested', tool: 'search', args: {} }), '', [], 'tid');
    expect(mcp.icon).toBeNull();
    expect(mcp.label).toBe('Tool consent requested: search');
  });

  it('engine/system-driven abort + auto-resume KEEP their chip (icon present)', () => {
    // Not user actions — the iconless treatment must NOT apply.
    const resume = describeInitiator(exchangeWith({ type: 'ContinuationStarted' }), '', [], 'tid');
    expect(resume.icon).not.toBeNull();
    expect(resume.label).toBe(ENGINE_LABEL);

    const crash = describeInitiator(exchangeWith({ type: 'ResponseAborted', actor: { kind: 'system' } }), '', [], 'tid');
    expect(crash.icon).not.toBeNull();
    expect(crash.label).toBe(SYSTEM_LABEL);
  });
});

// Recursively flatten a vnode tree to its visible text — the divider header
// `status` is a `<span>` vnode (label + optional glyph), not a bare string.
function textOf(node: ComponentChildren): string {
  if (node == null || node === false || node === true) return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(textOf).join('');
  return textOf((node as AnyVNode).props?.children);
}

function classOf(node: ComponentChildren): string {
  return ((node as AnyVNode)?.props?.class as string) ?? '';
}

const askedDivider = (steps: Exchange['steps'] = []): Exchange => ({
  userEvent: {
    type: 'UserQuestionAsked',
    tool_use_id: 'tu_div',
    cc_session_id: 'sess',
    question: 'Approve this plan?',
    options: [{ id: 'opt-0', label: 'Approve' }],
  } as Exchange['userEvent'],
  userSeq: 0,
  steps,
});

const sysAbort = (seq: number): Exchange['steps'][number] => ({
  seq,
  event: { type: 'ResponseAborted', cause: 'recovery_after_restart', actor: { kind: 'system' } } as Exchange['userEvent'],
});

// Regression: a question/permission divider whose turn ended WITHOUT the user
// answering must not claim the user "Canceled" it. A system abort (engine
// restart recovery) — like every other unanswered-terminal cause — reads as
// "Unanswered" / "Unresolved"; only an explicit user cancel reads "Canceled".
// The turn's actual terminal cause (Aborted ⚠ / Error ✕) is carried by the
// response panel + the abort boundary, not the divider header.
describe('describeInitiator — divider header reflects the QUESTION, not the turn', () => {
  it('system-aborted unanswered question reads "Unanswered", never "Canceled"', () => {
    const desc = describeInitiator(askedDivider([sysAbort(1)]), '', [], 'tid', /*responseTerminated*/ true, /*threadIsCC*/ true);
    expect(textOf(desc.status)).toContain('Unanswered');
    expect(textOf(desc.status)).not.toContain('Canceled');
  });

  it('agent-overtaken unanswered question reads "Unanswered" (terminated, no answer)', () => {
    const ex = askedDivider();
    ex.questionOvertaken = true;
    const desc = describeInitiator(ex, '', [], 'tid', /*responseTerminated*/ true, /*threadIsCC*/ true);
    expect(textOf(desc.status)).toContain('Unanswered');
    expect(textOf(desc.status)).not.toContain('Canceled');
  });

  it('user-canceled question still reads "Canceled ✕"', () => {
    const desc = describeInitiator(
      askedDivider([{ seq: 1, event: { type: 'UserQuestionAnswered', tool_use_id: 'tu_div', answer: { kind: 'Canceled' } } as Exchange['userEvent'] }]),
      '', [], 'tid', /*responseTerminated*/ true, /*threadIsCC*/ true,
    );
    expect(textOf(desc.status)).toContain('Canceled');
    expect(classOf(desc.status)).toContain('exchange-status-canceled');
  });

  it('answered question reads "Answered ✓" even with a trailing abort', () => {
    const desc = describeInitiator(
      askedDivider([
        { seq: 1, event: { type: 'UserQuestionAnswered', tool_use_id: 'tu_div', answer: { kind: 'Selected', option_id: 'opt-0' } } as Exchange['userEvent'] },
        sysAbort(2),
      ]),
      '', [], 'tid', /*responseTerminated*/ true, /*threadIsCC*/ true,
    );
    expect(textOf(desc.status)).toContain('Answered');
    expect(textOf(desc.status)).not.toContain('Unanswered');
  });

  it('pending (live) question reads "Needs your answer"', () => {
    const desc = describeInitiator(askedDivider(), '', [], 'tid', /*responseTerminated*/ false, /*threadIsCC*/ true);
    expect(textOf(desc.status)).toContain('Needs your answer');
  });

  it('system-aborted unresolved permission reads "Unresolved", never "Canceled"', () => {
    const ex: Exchange = {
      userEvent: {
        type: 'CodingAgentPermissionRequest',
        request_id: 'req-1',
        tool_use_id: 'tu-1',
        tool_name: 'Bash',
        input: {},
        summary: 'run a command',
      } as Exchange['userEvent'],
      userSeq: 0,
      steps: [sysAbort(1)],
    };
    const desc = describeInitiator(ex, '', [], 'tid', /*responseTerminated*/ true, /*threadIsCC*/ true);
    expect(textOf(desc.status)).toContain('Unresolved');
    expect(textOf(desc.status)).not.toContain('Canceled');
  });
});
