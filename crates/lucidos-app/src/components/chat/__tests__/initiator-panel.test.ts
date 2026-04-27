import { describe, it, expect } from 'vitest';
import { describeInitiator } from '../ChatExchange';
import { ENGINE_LABEL, LUCIDOS_AGENT_LABEL, type Exchange } from '../../../store/thread-events';

function exchangeWith(userEvent: Exchange['userEvent']): Exchange {
  return { userEvent, userSeq: 0, steps: [] };
}

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
    const desc = describeInitiator(ex, '<p>hello</p>', []);
    expect(desc.label).toBe('You');
    expect(desc.summary).toBeUndefined();
    expect(desc.variant).toBe('user');
  });

  it('agent-mode MessageReceived (parent_thread origin): label is "Lucidos Agent", summary "Forwarded message"', () => {
    // Parent thread's LLM kicked off this child via run_thread — the Lucidos
    // agent (not the engine itself) is the initiator, so the WHO label must be
    // "Lucidos Agent". The engine label stays for engine-internal triggers
    // (SessionRecovered, MissingHardeningDetected, scheduler with no human, …).
    const ex = exchangeWith({
      type: 'MessageReceived',
      text: '[Child thread completed] ...',
      mode: 'agent',
      channel: 'chat',
      origin: { kind: 'thread_link', thread_id: 'parent-1' },
    });
    const desc = describeInitiator(ex, '<p>...</p>', []);
    expect(desc.label).toBe(LUCIDOS_AGENT_LABEL);
    expect(desc.summary).toBe('Forwarded message');
    expect(desc.variant).toBe('system');
  });

  it('API-originated MessageReceived (human mode): chip is "You", summary "API message"', () => {
    // The chip is mode-driven: a human typed `curl` from a script, so the
    // initiator is still "You". The "API message" summary surfaces the
    // origin; the popover carries the user-agent detail.
    const ex = exchangeWith({
      type: 'MessageReceived',
      text: 'curl request',
      mode: 'human',
      channel: 'chat',
      origin: { kind: 'api', user_agent: 'curl/8.7.1' },
    });
    const desc = describeInitiator(ex, '<p>curl request</p>', []);
    expect(desc.label).toBe('You');
    expect(desc.summary).toBe('API message');
    expect(desc.variant).toBe('system');
  });

  it('SessionRecovered: label is engine, summary "Engine restarted"', () => {
    const ex = exchangeWith({ type: 'SessionRecovered' });
    const desc = describeInitiator(ex, '', []);
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBe('Engine restarted');
    expect(desc.variant).toBe('system');
  });

  it('MissingHardeningDetected: label is engine, summary "Hardening required"', () => {
    const ex = exchangeWith({ type: 'MissingHardeningDetected' });
    const desc = describeInitiator(ex, '', []);
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBe('Hardening required');
    expect(desc.variant).toBe('system');
  });

  it('MergeConflictDetected: label is engine, summary "Merging changes from main"', () => {
    const ex = exchangeWith({ type: 'MergeConflictDetected', files: ['a.rs', 'b.rs'] });
    const desc = describeInitiator(ex, '', []);
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBe('Merging changes from main');
    expect(desc.variant).toBe('system');
  });

  it('UserPromptInjected (no origin): label is engine, summary "Auto-prompt sent"', () => {
    const ex = exchangeWith({ type: 'UserPromptInjected', text: 'do X' });
    const desc = describeInitiator(ex, '<p>do X</p>', []);
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
    const desc = describeInitiator(ex, '<p>...</p>', []);
    expect(desc.label).toBe('Lucidos Agent');
    expect(desc.variant).toBe('system');
  });

  // Change lifecycle events: label is the actor (who did it), summary is the action.
  it('ChangeApplied (engine actor): label is engine, summary "Change applied"', () => {
    const ex = exchangeWith({
      type: 'ChangeApplied',
      change_id: 'c1',
      actor: { kind: 'engine', reason: { kind: 'session_recovered' } },
    });
    const desc = describeInitiator(ex, '', []);
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
    const desc = describeInitiator(ex, '', []);
    expect(desc.label).toBe('You');
    expect(desc.summary).toBe('Change applied');
  });

  it('ChangeApplied (no actor): defaults to engine label', () => {
    const ex = exchangeWith({ type: 'ChangeApplied', change_id: 'c1' });
    const desc = describeInitiator(ex, '', []);
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBe('Change applied');
  });

  it('ChangeDiscarded (device): label "You", summary "Change discarded"', () => {
    const ex = exchangeWith({
      type: 'ChangeDiscarded',
      change_id: 'c1',
      actor: { kind: 'device', device_id: 'd1', label: 'Mac' },
    });
    const desc = describeInitiator(ex, '', []);
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
    const desc = describeInitiator(ex, '', []);
    expect(desc.label).toBe('You');
    expect(desc.summary).toBe('Change reverted');
    expect(desc.accent).toBe('change-reverted');
  });

  it('ChangeApplyFailed: label is engine, summary "Change failed"', () => {
    const ex = exchangeWith({ type: 'ChangeApplyFailed', change_id: 'c1', error: 'boom' });
    const desc = describeInitiator(ex, '', []);
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
    const desc = describeInitiator(ex, '', []);
    expect(desc.label).toBe(ENGINE_LABEL);
    expect(desc.summary).toBe('Trigger fired');
    expect(desc.variant).toBe('trigger');
  });
});
