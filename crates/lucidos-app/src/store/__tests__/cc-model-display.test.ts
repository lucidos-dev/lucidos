import { describe, it, expect } from 'vitest';
import {
  groupIntoExchanges,
  exchangeResponseModel,
  exchangeReasoningEffort,
  displayModelName,
  displayReasoningEffort,
  type StoredEvent,
} from '../thread-events';

/** Build an events map from a [seq, event] list. */
function eventsMap(entries: [number, StoredEvent][]): Map<number, StoredEvent> {
  return new Map(entries);
}

describe('exchangeResponseModel for CC sessions', () => {
  it('returns model from CodingAgentSettingsChanged in a CC exchange', () => {
    const events = eventsMap([
      [1, { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: '2026-01-01T00:00:01Z' }],
      [2, { type: 'SessionStarted', session_id: 's1', created: '2026-01-01T00:00:02Z' }],
      [3, { type: 'CodingAgentSettingsChanged', model: 'claude-opus-4-6', created: '2026-01-01T00:00:03Z' }],
      [4, { type: 'CodingAgentToolCalled', name: 'Read', args: {}, created: '2026-01-01T00:00:04Z' }],
      [5, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok', created: '2026-01-01T00:00:05Z' }],
      [6, { type: 'CodingAgentIdled', has_changes: false, created: '2026-01-01T00:00:06Z' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(1);
    expect(exchangeResponseModel(exchanges[0])).toBe('claude-opus-4-6');
  });

  it('returns model from chat ResponseGenerated (existing behavior)', () => {
    const events = eventsMap([
      [1, { type: 'MessageReceived', text: 'hello', created: '2026-01-01T00:00:01Z' }],
      [2, { type: 'ResponseGenerated', text: 'hi', model: 'claude-sonnet-4-6', created: '2026-01-01T00:00:02Z' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchangeResponseModel(exchanges[0])).toBe('claude-sonnet-4-6');
  });

  it('prefers ResponseGenerated model over CodingAgentSettingsChanged', () => {
    // Edge case: both present (e.g. chat in a CC thread)
    const events = eventsMap([
      [1, { type: 'MessageReceived', text: 'test', created: '2026-01-01T00:00:01Z' }],
      [2, { type: 'CodingAgentSettingsChanged', model: 'claude-haiku-4-5-20251001', created: '2026-01-01T00:00:02Z' }],
      [3, { type: 'ResponseGenerated', text: 'result', model: 'claude-sonnet-4-6', created: '2026-01-01T00:00:03Z' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchangeResponseModel(exchanges[0])).toBe('claude-sonnet-4-6');
  });

  it('uses latest CodingAgentSettingsChanged when model changes mid-session', () => {
    const events = eventsMap([
      [1, { type: 'MessageReceived', text: 'fix it', channel: 'claude_code', created: '2026-01-01T00:00:01Z' }],
      [2, { type: 'SessionStarted', session_id: 's1', created: '2026-01-01T00:00:02Z' }],
      [3, { type: 'CodingAgentSettingsChanged', model: 'claude-sonnet-4-6', created: '2026-01-01T00:00:03Z' }],
      [4, { type: 'CodingAgentSettingsChanged', model: 'claude-opus-4-6', created: '2026-01-01T00:00:04Z' }],
      [5, { type: 'CodingAgentIdled', has_changes: false, created: '2026-01-01T00:00:05Z' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchangeResponseModel(exchanges[0])).toBe('claude-opus-4-6');
  });

  it('returns model from ResponseGenerated in a CC exchange (default model)', () => {
    // When user doesn't select a model, the engine puts the full model ID
    // on ResponseGenerated (normalized from CC's alias).
    const events = eventsMap([
      [1, { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: '2026-01-01T00:00:01Z' }],
      [2, { type: 'SessionStarted', session_id: 's1', created: '2026-01-01T00:00:02Z' }],
      [3, { type: 'CodingAgentToolCalled', name: 'Read', args: {}, created: '2026-01-01T00:00:03Z' }],
      [4, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok', created: '2026-01-01T00:00:04Z' }],
      [5, { type: 'ResponseGenerated', text: 'done', model: 'claude-opus-4-6', created: '2026-01-01T00:00:05Z' }],
      [6, { type: 'CodingAgentIdled', has_changes: false, created: '2026-01-01T00:00:06Z' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(1);
    expect(exchangeResponseModel(exchanges[0])).toBe('claude-opus-4-6');
  });

  it('returns undefined when CodingAgentSettingsChanged has no model field', () => {
    const events = eventsMap([
      [1, { type: 'MessageReceived', text: 'fix it', channel: 'claude_code', created: '2026-01-01T00:00:01Z' }],
      [2, { type: 'SessionStarted', session_id: 's1', created: '2026-01-01T00:00:02Z' }],
      [3, { type: 'CodingAgentSettingsChanged', reasoning_effort: 'high', created: '2026-01-01T00:00:03Z' }],
      [4, { type: 'CodingAgentIdled', has_changes: false, created: '2026-01-01T00:00:04Z' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchangeResponseModel(exchanges[0])).toBeUndefined();
  });

  it('returns undefined for idle CC exchange without CodingAgentSettingsChanged (default model, no Init emit)', () => {
    // Bug scenario: user didn't select a model, CC resolved it via Init, but
    // CodingAgentSettingsChanged was never emitted. Idle exchange has no model source.
    const events = eventsMap([
      [1, { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: '2026-01-01T00:00:01Z' }],
      [2, { type: 'SessionStarted', session_id: 's1', created: '2026-01-01T00:00:02Z' }],
      [3, { type: 'CodingAgentToolCalled', name: 'Read', args: {}, created: '2026-01-01T00:00:03Z' }],
      [4, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok', created: '2026-01-01T00:00:04Z' }],
      [5, { type: 'CodingAgentIdled', has_changes: false, created: '2026-01-01T00:00:05Z' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(1);
    // Without CodingAgentSettingsChanged or ResponseGenerated, no model is available
    expect(exchangeResponseModel(exchanges[0])).toBeUndefined();
  });

  it('returns model for idle CC exchange when CodingAgentSettingsChanged is emitted after Init', () => {
    // Fixed scenario: Init resolves default model and emits CodingAgentSettingsChanged.
    // Even without ResponseGenerated, idle exchange shows the model.
    const events = eventsMap([
      [1, { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: '2026-01-01T00:00:01Z' }],
      [2, { type: 'SessionStarted', session_id: 's1', created: '2026-01-01T00:00:02Z' }],
      [3, { type: 'CodingAgentSettingsChanged', model: 'claude-opus-4-6', created: '2026-01-01T00:00:03Z' }],
      [4, { type: 'CodingAgentToolCalled', name: 'Read', args: {}, created: '2026-01-01T00:00:04Z' }],
      [5, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok', created: '2026-01-01T00:00:05Z' }],
      [6, { type: 'CodingAgentIdled', has_changes: false, created: '2026-01-01T00:00:06Z' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(1);
    expect(exchangeResponseModel(exchanges[0])).toBe('claude-opus-4-6');
  });

  it('returns model for CC follow-up exchange (CodingAgentSettingsChanged re-emitted)', () => {
    // Follow-up exchanges get their own CodingAgentSettingsChanged from run_direct_claude_code.
    const events = eventsMap([
      // First exchange
      [1, { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: '2026-01-01T00:00:01Z' }],
      [2, { type: 'SessionStarted', session_id: 's1', created: '2026-01-01T00:00:02Z' }],
      [3, { type: 'CodingAgentSettingsChanged', model: 'claude-opus-4-6', created: '2026-01-01T00:00:03Z' }],
      [4, { type: 'ResponseGenerated', text: 'done', model: 'claude-opus-4-6', created: '2026-01-01T00:00:04Z' }],
      [5, { type: 'CodingAgentIdled', has_changes: false, created: '2026-01-01T00:00:05Z' }],
      // Follow-up exchange
      [6, { type: 'MessageReceived', text: 'now add tests', channel: 'claude_code', created: '2026-01-01T00:00:06Z' }],
      [7, { type: 'SessionStarted', session_id: 's2', created: '2026-01-01T00:00:07Z' }],
      [8, { type: 'CodingAgentSettingsChanged', model: 'claude-opus-4-6', created: '2026-01-01T00:00:08Z' }],
      [9, { type: 'CodingAgentIdled', has_changes: true, created: '2026-01-01T00:00:09Z' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    // First exchange: model from ResponseGenerated
    expect(exchangeResponseModel(exchanges[0])).toBe('claude-opus-4-6');
    // Follow-up exchange: model from CodingAgentSettingsChanged (no ResponseGenerated yet)
    expect(exchangeResponseModel(exchanges[1])).toBe('claude-opus-4-6');
  });

  it('returns model from MessageReceived for in-progress chat exchange (no ResponseGenerated yet)', () => {
    // Bug: while a chat response is "Working", model/effort were missing from the
    // route tooltip because they only appear on terminal events. The chat path
    // now stamps them on MessageReceived so they're visible immediately.
    const events = eventsMap([
      [1, { type: 'MessageReceived', text: 'hi', model: 'claude-opus-4-6', reasoning_effort: 'high', created: '2026-01-01T00:00:01Z' }],
      [2, { type: 'TextStreamed', text: 'hel', created: '2026-01-01T00:00:02Z' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchangeResponseModel(exchanges[0])).toBe('claude-opus-4-6');
    expect(exchangeReasoningEffort(exchanges[0])).toBe('high');
  });

  it('prefers ResponseGenerated over MessageReceived once response completes', () => {
    const events = eventsMap([
      [1, { type: 'MessageReceived', text: 'hi', model: 'claude-opus-4-6', reasoning_effort: 'low', created: '2026-01-01T00:00:01Z' }],
      [2, { type: 'ResponseGenerated', text: 'done', model: 'claude-sonnet-4-6', reasoning_effort: 'high', created: '2026-01-01T00:00:02Z' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchangeResponseModel(exchanges[0])).toBe('claude-sonnet-4-6');
    expect(exchangeReasoningEffort(exchanges[0])).toBe('high');
  });

  it('skips terminal events without model and falls through to CodingAgentSettingsChanged', () => {
    // CC follow-up where ResponseAborted has no model (recovery/shutdown path)
    // but CodingAgentSettingsChanged in the same exchange has it.
    const events = eventsMap([
      [1, { type: 'MessageReceived', text: 'fix it', channel: 'claude_code', created: '2026-01-01T00:00:01Z' }],
      [2, { type: 'CodingAgentSettingsChanged', model: 'claude-opus-4-7', created: '2026-01-01T00:00:02Z' }],
      [3, { type: 'CodingAgentToolCalled', name: 'Read', args: {}, created: '2026-01-01T00:00:03Z' }],
      [4, { type: 'ResponseAborted', text: 'partial', created: '2026-01-01T00:00:04Z' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchangeResponseModel(exchanges[0])).toBe('claude-opus-4-7');
  });
});

describe('displayModelName', () => {
  it('maps known full model IDs to friendly labels', () => {
    expect(displayModelName('claude-opus-4-6')).toBe('Opus 4.6');
    expect(displayModelName('claude-sonnet-4-6')).toBe('Sonnet 4.6');
    expect(displayModelName('claude-opus-4-6[1m]')).toBe('Opus 4.6 (1M)');
    expect(displayModelName('claude-haiku-4-5-20251001')).toBe('Haiku 4.5');
    expect(displayModelName('claude-opus-4-1')).toBe('Opus 4.1');
    expect(displayModelName('claude-opus-4-5@20251101')).toBe('Opus 4.5');
  });

  it('maps non-Claude model IDs to friendly labels', () => {
    expect(displayModelName('gpt-5.4')).toBe('GPT-5.4');
    expect(displayModelName('gemini-3.1-pro-preview')).toBe('Gemini 3.1 Pro');
  });

  it('passes through unknown model IDs unchanged', () => {
    expect(displayModelName('some-custom-model')).toBe('some-custom-model');
  });

  it('maps legacy short aliases to versioned labels', () => {
    // Old events stored before the migration had short aliases.
    // These are historical facts — opus was 4.6, sonnet was 4.6, haiku was 4.5.
    expect(displayModelName('opus')).toBe('Opus 4.6');
    expect(displayModelName('sonnet')).toBe('Sonnet 4.6');
    expect(displayModelName('haiku')).toBe('Haiku 4.5');
  });
});

describe('exchangeReasoningEffort', () => {
  it('returns effort from ResponseGenerated', () => {
    const events = eventsMap([
      [1, { type: 'MessageReceived', text: 'hello', created: '2026-01-01T00:00:01Z' }],
      [2, { type: 'ResponseGenerated', text: 'hi', model: 'claude-opus-4-6', reasoning_effort: 'high', created: '2026-01-01T00:00:02Z' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchangeReasoningEffort(exchanges[0])).toBe('high');
  });

  it('returns effort from CodingAgentSettingsChanged in CC exchange', () => {
    const events = eventsMap([
      [1, { type: 'MessageReceived', text: 'fix', channel: 'claude_code', created: '2026-01-01T00:00:01Z' }],
      [2, { type: 'SessionStarted', session_id: 's1', created: '2026-01-01T00:00:02Z' }],
      [3, { type: 'CodingAgentSettingsChanged', model: 'claude-opus-4-6', reasoning_effort: 'max', created: '2026-01-01T00:00:03Z' }],
      [4, { type: 'CodingAgentIdled', has_changes: false, created: '2026-01-01T00:00:04Z' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchangeReasoningEffort(exchanges[0])).toBe('max');
  });

  it('prefers ResponseGenerated effort over CodingAgentSettingsChanged', () => {
    const events = eventsMap([
      [1, { type: 'MessageReceived', text: 'test', created: '2026-01-01T00:00:01Z' }],
      [2, { type: 'CodingAgentSettingsChanged', reasoning_effort: 'low', created: '2026-01-01T00:00:02Z' }],
      [3, { type: 'ResponseGenerated', text: 'result', reasoning_effort: 'high', created: '2026-01-01T00:00:03Z' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchangeReasoningEffort(exchanges[0])).toBe('high');
  });

  it('returns undefined when no effort is set', () => {
    const events = eventsMap([
      [1, { type: 'MessageReceived', text: 'hello', created: '2026-01-01T00:00:01Z' }],
      [2, { type: 'ResponseGenerated', text: 'hi', created: '2026-01-01T00:00:02Z' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchangeReasoningEffort(exchanges[0])).toBeUndefined();
  });
});

describe('displayReasoningEffort', () => {
  it('maps known effort values to labels', () => {
    expect(displayReasoningEffort('none')).toBe('Off');
    expect(displayReasoningEffort('low')).toBe('Low');
    expect(displayReasoningEffort('medium')).toBe('Med');
    expect(displayReasoningEffort('high')).toBe('High');
    expect(displayReasoningEffort('max')).toBe('Max');
  });

  it('passes through unknown effort values unchanged', () => {
    expect(displayReasoningEffort('turbo')).toBe('turbo');
  });
});
