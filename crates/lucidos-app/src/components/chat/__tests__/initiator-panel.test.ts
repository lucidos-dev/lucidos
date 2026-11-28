import { describe, it, expect } from 'vitest';
import { actorInitiator, describeInitiator } from '../ChatExchange';
import { renderOriginSection, renderChannelSection, technicalDetailRows } from '../MessageRoutePanel';
import { ENGINE_LABEL, LUCIDOS_AGENT_LABEL, SYSTEM_LABEL, type Exchange } from '../../../store/thread-events';
import type { ComponentChildren, VNode } from 'preact';
import { vnodeToText } from './vnodeToText';

function exchangeWith(userEvent: Exchange['userEvent']): Exchange {
  return { userEvent, userSeq: 0, steps: [] };
}

interface AnyVNode extends VNode<{ children?: ComponentChildren; class?: string; [k: string]: unknown }> {}

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

  it('agent-mode MessageReceived (parent_thread origin): label is "Lucidos Agent", summary names the sending thread, lucidos variant', () => {
    const ex = exchangeWith({
      type: 'MessageReceived',
      text: '[Child thread completed] ...',
      mode: 'agent',
      channel: 'chat',
      origin: { kind: 'thread_link', thread_id: 'parent-1' },
    });
    const desc = describeInitiator(ex, '<p>...</p>', [], 'tid');
    expect(desc.label).toBe(LUCIDOS_AGENT_LABEL);
    expect(desc.summary).toBe('Message from another thread');
    expect(desc.variant).toBe('lucidos');
  });

  it('engine-seeded MessageReceived: the summary names the plugin, not "Forwarded message"', () => {
    const ex = exchangeWith({
      type: 'MessageReceived',
      text: 'Set up ⏰ Habit Tracker again: its setup instructions changed since version 0.1.3.',
      mode: 'engine',
      channel: 'chat',
      origin: {
        kind: 'engine',
        reason: {
          kind: 'plugin_setup', plugin_id: 'habit-tracker', plugin_name: '⏰ Habit Tracker', version: '0.1.4',
          occasion: { kind: 'update', from_version: '0.1.3' },
        },
      },
    });
    const desc = describeInitiator(ex, '<p>x</p>', [], 'tid');
    expect(desc.label).toBe('Lucidos Engine');
    expect(desc.summary).toBe('Plugin update: ⏰ Habit Tracker 0.1.3 → 0.1.4');
  });

  it('agent-mode MessageReceived with no sender on record: summary "Message from an agent"', () => {
    const ex = exchangeWith({ type: 'MessageReceived', text: 'x', mode: 'agent', channel: 'chat' });
    expect(describeInitiator(ex, '<p>x</p>', [], 'tid').summary).toBe('Message from an agent');
  });

  it('a delivered held message names its sender and says it waited (ADR 0256)', () => {
    const ex: Exchange = {
      ...exchangeWith({
        type: 'MessageReceived',
        text: 'Stop now',
        mode: 'agent',
        channel: 'claude_code',
        origin: { kind: 'thread_link', thread_id: 'parent-1', title: 'Loop master' },
      }),
      releasedFromHold: true,
    };
    expect(describeInitiator(ex, '<p>Stop now</p>', [], 'tid').summary)
      .toBe('Message from "Loop master", held until you replied');
  });

  it('API-originated MessageReceived (human mode): chip is "API caller", summary "API message"', () => {
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

  it('unattested MessageReceived (human mode): never "You", even with a claimed device', () => {
    const ex = exchangeWith({
      type: 'MessageReceived',
      text: 'typed device',
      mode: 'human',
      channel: 'chat',
      origin: { kind: 'unattested_caller', mode: 'human', claimed_device_id: 'owner-phone' },
    });
    const desc = describeInitiator(ex, '<p>typed device</p>', [], 'tid');
    expect(desc.label).toBe('A program that named a device');
    expect(desc.summary).toBe('Message from a program that named a device');
    expect(desc.variant).toBe('system');
  });

  it('unattested caller with the local token: the chip names a local program', () => {
    const userEvent: Exchange['userEvent'] = {
      type: 'MessageReceived',
      text: 'from a terminal',
      mode: 'engine',
      channel: 'chat',
      origin: { kind: 'unattested_caller', mode: 'engine', claimed_local_token: true, user_agent: 'lucidos-cli' },
    };
    const desc = describeInitiator(exchangeWith(userEvent), '<p>from a terminal</p>', [], 'tid');
    expect(desc.label).toBe('A program on this computer');
    expect(technicalDetailRows(userEvent, userEvent.origin)).toEqual([
      ['Event type', 'MessageReceived'],
      ['Claimed credential', 'Machine-local token'],
      ['Client', 'lucidos-cli'],
    ]);
  });

  it('a call from another workspace: its claim or its verdict is in the technical details', () => {
    const unstamped: Exchange['userEvent'] = {
      type: 'MessageReceived',
      text: 'spawn',
      mode: 'agent',
      channel: 'chat',
      origin: { kind: 'unattested_caller', mode: 'agent', claimed_workspace: 'work2' },
    };
    expect(technicalDetailRows(unstamped, unstamped.origin)).toEqual([
      ['Event type', 'MessageReceived'],
      ['Claimed workspace', 'work2'],
    ]);
    const refused: Exchange['userEvent'] = {
      ...unstamped,
      origin: {
        kind: 'workspace',
        workspace: 'work2',
        mode: 'agent',
        verdict: { kind: 'refused', reason: 'No grant.' },
      },
    };
    expect(technicalDetailRows(refused, refused.origin)).toEqual([
      ['Event type', 'MessageReceived'],
      ['Its gate said', 'Refused: No grant.'],
    ]);
  });

  it('ContinuationStarted (no actor): label falls back to engine, summary "Resumed after engine restart"', () => {
    const ex = exchangeWith({ type: 'ContinuationStarted' });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBe('Resumed after engine restart');
    expect(desc.variant).toBe('system');
  });

  it('ContinuationStarted (device actor): iconless action label "Continued the response" (ResponseCanceled style)', () => {
    const ex = exchangeWith({
      type: 'ContinuationStarted',
      actor: { kind: 'device', device_id: 'd-1' },
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.icon).toBeNull();
    expect(desc.label).toBe('Continued the response');
    expect(desc.summary).toBeUndefined();
    expect(desc.variant).toBe('system');
  });

  it('ContinuationStarted (auto_recovery_after_hang): NOT labeled an engine restart', () => {
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
    const ex = exchangeWith({
      type: 'ResponseAborted',
      cause: 'engine_shutdown',
      actor: { kind: 'device', device_id: 'd-1' },
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.icon).toBeNull();
    expect(desc.label).toBe('Paused by restart');
    expect(desc.summary).toBeUndefined();
  });

  it('ResponseAborted (device actor, no cause): reads as an interruption, not a pause', () => {
    const ex = exchangeWith({
      type: 'ResponseAborted',
      actor: { kind: 'device', device_id: 'd-1' },
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.icon).toBeNull();
    expect(desc.label).toBe('Response interrupted');
  });

  it('EventWaitCanceled (user stop): iconless action label naming what was stopped', () => {
    const ex = exchangeWith({
      type: 'EventWaitCanceled',
      wait_id: 'w1',
      cause: 'user_stop',
      reason: "tonight's E2E suite to pass",
      actor: { kind: 'device', device_id: 'd-1' },
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.icon).toBeNull();
    expect(desc.label).toBe("Stopped waiting for tonight's E2E suite to pass");
    expect(desc.summary).toBeUndefined();
    expect(desc.actorClickable).not.toBe(false);
  });

  it('EventWaitCanceled (user stop, legacy row): names no subscription it cannot name', () => {
    const ex = exchangeWith({ type: 'EventWaitCanceled', wait_id: 'w1', cause: 'user_stop' });
    expect(describeInitiator(ex, '', [], 'tid').label).toBe('Stopped waiting for an event');
  });

  it('ResponseCanceled: no chip — "Response canceled" IS the header label, no summary, no icon', () => {
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
      actor: { kind: 'device', device_id: 'd1' },
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe('Response canceled');
    expect(desc.icon).toBeNull();
    expect(desc.summary).toBeUndefined();
  });

  it('MissingHardeningDetected: label is engine, summary "Hardening needed"', () => {
    const ex = exchangeWith({ type: 'MissingHardeningDetected' });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBe('Hardening needed');
    expect(desc.variant).toBe('system');
  });

  it('MergeConflictDetected: label is engine, summary "Merge conflict"', () => {
    const ex = exchangeWith({ type: 'MergeConflictDetected', files: ['a.rs', 'b.rs'] });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBe('Merge conflict');
    expect(desc.variant).toBe('system');
  });

  it('PromptInjected (no origin): label is engine, summary "Message from Lucidos"', () => {
    const ex = exchangeWith({ type: 'PromptInjected', text: 'do X' });
    const desc = describeInitiator(ex, '<p>do X</p>', [], 'tid');
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBe('Message from Lucidos');
    expect(desc.variant).toBe('system');
  });

  it('PromptInjected (event delivery): the row names the event, the header stays quiet', () => {
    const ex = exchangeWith({
      type: 'PromptInjected',
      text: 'An event you subscribed to has arrived …\n\nCodingAgentIdled:\n{ … }',
      delivered_event_id: 'evt-1',
    });
    const desc = describeInitiator(
      ex, '<p>raw json</p>', [], 'tid', false, false, 'claude-code', {},
      { eventType: 'CodingAgentIdled', eventId: 'evt-src-1', payloadJson: '{\n  "has_changes": true\n}' },
    );
    expect(desc.summary).toBeUndefined();
    const body = desc.details as { type?: { name?: string }; props?: Record<string, unknown> };
    expect(body?.type?.name).toBe('EventDeliveryBody');
    expect(body?.props).toMatchObject({
      eventType: 'CodingAgentIdled',
      eventId: 'evt-src-1',
      payloadJson: '{\n  "has_changes": true\n}',
    });
  });

  it('PromptInjected (delivery not loaded): falls back to the prose body', () => {
    const ex = exchangeWith({
      type: 'PromptInjected',
      text: 'An event you subscribed to has arrived …',
      delivered_event_id: 'evt-gone',
    });
    const desc = describeInitiator(ex, '<p>raw json</p>', [], 'tid');
    expect(desc.summary).toBe('Event arrived');
    expect(desc.details).toBeDefined();
  });

  it('PromptInjected (timed-out wait, older row): summary "Wait timed out"', () => {
    const ex = exchangeWith({
      type: 'PromptInjected',
      text: 'A subscription you registered has timed out.\n\nTimed out. Nothing matching X happened.',
      mode: 'agent',
    });
    expect(describeInitiator(ex, '<p>x</p>', [], 'tid').summary).toBe('Wait timed out');
  });

  it('PromptInjected (timed-out wait, engine origin): summary "Wait timed out"', () => {
    const ex = exchangeWith({
      type: 'PromptInjected',
      text: 'anything',
      mode: 'agent',
      origin: { kind: 'engine', reason: { kind: 'event_wait', outcome: 'expired', watched: ['X'], wait_reason: 'r' } },
    });
    const desc = describeInitiator(ex, '<p>x</p>', [], 'tid');
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBe('Wait timed out');
  });

  it('PromptInjected (child callback): label is Lucidos Agent', () => {
    const ex = exchangeWith({
      type: 'PromptInjected',
      text: '[Child thread completed] ...',
      mode: 'agent',
      origin: { kind: 'thread_link', thread_id: 'child-1', mode: 'agent', direction: 'child' },
    });
    const desc = describeInitiator(ex, '<p>...</p>', [], 'tid');
    expect(desc.label).toBe('Lucidos Agent');
    expect(desc.variant).toBe('lucidos');
  });

  it('ChangeApplied (engine actor): label is engine, change event row', () => {
    const ex = exchangeWith({
      type: 'ChangeApplied',
      change_id: 'c1',
      actor: { kind: 'engine', reason: { kind: 'session_recovered' } },
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBeUndefined();
    expect((desc.details as { type?: { name?: string } })?.type?.name).toBe('ChangeEventRow');
    expect(desc.accent).toBe('change-applied');
  });

  it('ChangeApplied (device actor): label "You", change event row', () => {
    const ex = exchangeWith({
      type: 'ChangeApplied',
      change_id: 'c1',
      actor: { kind: 'device', device_id: 'd1' },
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe('You');
    expect(desc.summary).toBeUndefined();
    expect((desc.details as { type?: { name?: string } })?.type?.name).toBe('ChangeEventRow');
  });

  it('ChangeApplied (no actor): defaults to engine label', () => {
    const ex = exchangeWith({ type: 'ChangeApplied', change_id: 'c1' });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBeUndefined();
    expect((desc.details as { type?: { name?: string } })?.type?.name).toBe('ChangeEventRow');
  });

  it('ChangeDiscarded (device): label "You", change event row', () => {
    const ex = exchangeWith({
      type: 'ChangeDiscarded',
      change_id: 'c1',
      actor: { kind: 'device', device_id: 'd1' },
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe('You');
    expect(desc.summary).toBeUndefined();
    expect((desc.details as { type?: { name?: string } })?.type?.name).toBe('ChangeEventRow');
    expect(desc.accent).toBe('change-discarded');
  });

  it('ChangeReverted (device): label "You", change event row', () => {
    const ex = exchangeWith({
      type: 'ChangeReverted',
      change_id: 'c1',
      actor: { kind: 'device', device_id: 'd1' },
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe('You');
    expect(desc.summary).toBeUndefined();
    expect((desc.details as { type?: { name?: string } })?.type?.name).toBe('ChangeEventRow');
    expect(desc.accent).toBe('change-reverted');
  });

  it('ChangeApplyFailed: label is engine, change event row', () => {
    const ex = exchangeWith({ type: 'ChangeApplyFailed', change_id: 'c1', error: 'boom' });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBeUndefined();
    expect((desc.details as { type?: { name?: string } })?.type?.name).toBe('ChangeEventRow');
    expect(desc.accent).toBe('change-failed');
  });

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

describe('describeInitiator — user control turns render iconless (ResponseCanceled style)', () => {
  it('PromptInjected (device origin): iconless label names it as your message, injected body kept', () => {
    const ex = exchangeWith({
      type: 'PromptInjected',
      text: 'do X',
      origin: { kind: 'device', device_id: 'd-1' },
    });
    const desc = describeInitiator(ex, '<p>do X</p>', [], 'tid');
    expect(desc.icon).toBeNull();
    expect(desc.label).toBe('Your message, read while the agent worked');
    expect(desc.summary).toBeUndefined();
    expect(desc.details).toBeDefined();
  });

  it('McpConsentRequested: iconless label = the request summary', () => {
    const mcp = describeInitiator(exchangeWith({ type: 'McpConsentRequested', tool: 'search', args: {} }), '', [], 'tid');
    expect(mcp.icon).toBeNull();
    expect(mcp.label).toBe('Tool consent requested: search');
  });

  it('engine/system-driven abort + auto-resume KEEP their chip (icon present)', () => {
    const resume = describeInitiator(exchangeWith({ type: 'ContinuationStarted' }), '', [], 'tid');
    expect(resume.icon).not.toBeNull();
    expect(resume.label).toBe(ENGINE_LABEL);

    const crash = describeInitiator(exchangeWith({ type: 'ResponseAborted', actor: { kind: 'system' } }), '', [], 'tid');
    expect(crash.icon).not.toBeNull();
    expect(crash.label).toBe(SYSTEM_LABEL);
  });
});

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

describe('describeInitiator — divider header reflects the QUESTION, not the turn', () => {
  it('system-aborted unanswered question reads "Unanswered", never "Canceled"', () => {
    const desc = describeInitiator(askedDivider([sysAbort(1)]), '', [], 'tid',  true,  true);
    expect(textOf(desc.status)).toContain('Unanswered');
    expect(textOf(desc.status)).not.toContain('Canceled');
  });

  it('agent-overtaken unanswered question reads "Unanswered" (terminated, no answer)', () => {
    const ex = askedDivider();
    ex.questionOvertaken = true;
    const desc = describeInitiator(ex, '', [], 'tid',  true,  true);
    expect(textOf(desc.status)).toContain('Unanswered');
    expect(textOf(desc.status)).not.toContain('Canceled');
  });

  it('user-canceled question still reads "Canceled ✕"', () => {
    const desc = describeInitiator(
      askedDivider([{ seq: 1, event: { type: 'UserQuestionAnswered', tool_use_id: 'tu_div', answer: { kind: 'Canceled' } } as Exchange['userEvent'] }]),
      '', [], 'tid',  true,  true,
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
      '', [], 'tid',  true,  true,
    );
    expect(textOf(desc.status)).toContain('Answered');
    expect(textOf(desc.status)).not.toContain('Unanswered');
  });

  it('pending (live) question reads "Needs your answer"', () => {
    const desc = describeInitiator(askedDivider(), '', [], 'tid',  false,  true);
    expect(vnodeToText(desc.status)).toContain('Needs your answer');
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
    const desc = describeInitiator(ex, '', [], 'tid',  true,  true);
    expect(textOf(desc.status)).toContain('Unresolved');
    expect(textOf(desc.status)).not.toContain('Canceled');
  });
});

describe('unsigned caller disclosure', () => {
  it('names the workspace claim outside Technical details and offers pairing', () => {
    const origin = { kind: 'unattested_caller' as const, mode: 'agent' as const, claimed_workspace: 'studio' };
    const exchange = exchangeWith({ type: 'MessageReceived', text: 'hello', channel: 'chat', mode: 'agent', origin });
    const desc = describeInitiator(exchange, '<p>hello</p>', [], 'tid');
    expect(desc.label).toBe('Workspace "studio"');
    expect(desc.summary).toBe('Message from workspace "studio", by its own account');
    expect(actorInitiator(origin).icon).toBeNull();
    const channel = vnodeToText(renderChannelSection(origin));
    expect(channel).toContain('Sent from');
    expect(channel).toContain('Workspace "studio", by its own account');
    const section = renderOriginSection(exchange, undefined, () => undefined);
    const children = (section.props.children as ComponentChildren[]).filter(child =>
      !child || typeof child !== 'object' || Array.isArray(child)
      || (child as AnyVNode).props?.class !== 'route-technical');
    const body = vnodeToText(children);
    expect(body).toContain('How Lucidos can check it');
    expect(body).toContain('Pair a gateway in Settings');
    expect(body).toContain('If the workspaces use separate gateways, pair them first.');
    expect(body).toContain('calls sent by a thread through its gateway');
    expect(body).not.toMatch(/unattested|not verified|unconfirmed/i);
  });

  it('keeps unknown callers unknown and device names explicitly claimed', () => {
    expect(actorInitiator({ kind: 'unattested_caller' }).label).toBe('An unknown caller');
    expect(vnodeToText(renderChannelSection({ kind: 'unattested_caller', claimed_local_token: true })))
      .toContain('A program on this computer');
    expect(actorInitiator({ kind: 'unattested_caller', claimed_device_id: 'typed-id' }).label)
      .toContain('A program that named');
  });
});
