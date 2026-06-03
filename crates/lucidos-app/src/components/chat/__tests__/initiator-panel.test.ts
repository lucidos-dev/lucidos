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
    // Legacy DB rows / auto-resume case carry no actor; chip falls back to ⚙ Engine.
    const ex = exchangeWith({ type: 'ContinuationStarted' });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBe('Resumed after engine restart');
    expect(desc.variant).toBe('system');
  });

  it('ContinuationStarted (device actor): chip is "You"', () => {
    const ex = exchangeWith({
      type: 'ContinuationStarted',
      actor: { kind: 'device', device_id: 'd-1', label: 'My Mac' },
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe('You');
    expect(desc.variant).toBe('system');
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

  it('ResponseAborted (device actor = restart pre-emit): chip is "You", summary "Restarted"', () => {
    // /api/v1/restart → abort_in_flight_for_restart pre-emits with the device
    // actor that hit Restart. The chip + summary should read together as
    // "You — Restarted", not the generic "Response interrupted".
    const ex = exchangeWith({
      type: 'ResponseAborted',
      actor: { kind: 'device', device_id: 'd-1', label: 'iOS Safari PWA' },
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe('You');
    expect(desc.summary).toBe('Restarted');
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

  it('TriggerStarted: label is engine, summary "Trigger fired" (trigger name surfaces in the popover Origin row)', () => {
    const ex = exchangeWith({
      type: 'TriggerStarted',
      trigger_id: 't1',
      trigger_name: 'morning-summary',
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBe('Trigger fired');
    expect(desc.variant).toBe('trigger');
  });

  it('ChildThreadCompleted: label is "Lucidos Agent", lucidos variant, no panel summary line (card owns the prefix)', () => {
    const ex = exchangeWith({
      type: 'ChildThreadCompleted',
      child_thread_id: 'child-1',
      child_thread_title: 'Refactor foo',
      status: 'success',
      summary: 'cleaned up.',
      pending_change_ids: [],
    });
    const desc = describeInitiator(ex, '', [], 'tid');
    expect(desc.label).toBe(LUCIDOS_AGENT_LABEL);
    expect(desc.summary).toBeUndefined();
    expect(desc.variant).toBe('lucidos');
  });

  it('ChildThreadCompleted: details embed the ChildCompletionCard with child_thread_id, title, status, and summary', () => {
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
    expect((node!.type as { name?: string }).name).toBe('ChildCompletionCard');
    const props = node!.props as Record<string, unknown>;
    expect(props.childThreadId).toBe('child-1');
    expect(props.childThreadTitle).toBe('Refactor foo');
    expect(props.status).toBe('success');
    expect(props.summary).toBe('done.');
  });
});
