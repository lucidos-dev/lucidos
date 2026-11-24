import { describe, it, expect } from 'vitest';
import {
  groupIntoExchanges,
  exchangeResponseModel,
  exchangeReasoningEffort,
  displayModelName,
  displayReasoningEffort,
  type StoredEvent,
} from '../thread-events';

function eventsMap(entries: [number, StoredEvent][]): Map<number, StoredEvent> {
  return new Map(entries);
}

describe('exchangeResponseModel for Claude Code sessions', () => {
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
    const events = eventsMap([
      [1, { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: '2026-01-01T00:00:01Z' }],
      [2, { type: 'SessionStarted', session_id: 's1', created: '2026-01-01T00:00:02Z' }],
      [3, { type: 'CodingAgentToolCalled', name: 'Read', args: {}, created: '2026-01-01T00:00:03Z' }],
      [4, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok', created: '2026-01-01T00:00:04Z' }],
      [5, { type: 'CodingAgentIdled', has_changes: false, created: '2026-01-01T00:00:05Z' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(1);
    expect(exchangeResponseModel(exchanges[0])).toBeUndefined();
  });

  it('returns model for idle CC exchange when CodingAgentSettingsChanged is emitted after Init', () => {
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
    const events = eventsMap([
      [1, { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: '2026-01-01T00:00:01Z' }],
      [2, { type: 'SessionStarted', session_id: 's1', created: '2026-01-01T00:00:02Z' }],
      [3, { type: 'CodingAgentSettingsChanged', model: 'claude-opus-4-6', created: '2026-01-01T00:00:03Z' }],
      [4, { type: 'ResponseGenerated', text: 'done', model: 'claude-opus-4-6', created: '2026-01-01T00:00:04Z' }],
      [5, { type: 'CodingAgentIdled', has_changes: false, created: '2026-01-01T00:00:05Z' }],
      [6, { type: 'MessageReceived', text: 'now add tests', channel: 'claude_code', created: '2026-01-01T00:00:06Z' }],
      [7, { type: 'SessionStarted', session_id: 's2', created: '2026-01-01T00:00:07Z' }],
      [8, { type: 'CodingAgentSettingsChanged', model: 'claude-opus-4-6', created: '2026-01-01T00:00:08Z' }],
      [9, { type: 'CodingAgentIdled', has_changes: true, created: '2026-01-01T00:00:09Z' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    expect(exchangeResponseModel(exchanges[0])).toBe('claude-opus-4-6');
    expect(exchangeResponseModel(exchanges[1])).toBe('claude-opus-4-6');
  });

  it('returns model from MessageReceived for in-progress chat exchange (no ResponseGenerated yet)', () => {
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
    expect(displayModelName('claude-opus-4-7')).toBe('Opus 4.7');
    expect(displayModelName('claude-opus-4-7[1m]')).toBe('Opus 4.7 (1M)');
    expect(displayModelName('claude-opus-4-6')).toBe('Opus 4.6');
    expect(displayModelName('claude-opus-4-6[1m]')).toBe('Opus 4.6 (1M)');
    expect(displayModelName('claude-sonnet-5')).toBe('Sonnet 5');
    expect(displayModelName('claude-sonnet-5[1m]')).toBe('Sonnet 5 (1M)');
    expect(displayModelName('claude-sonnet-4-6')).toBe('Sonnet 4.6');
    expect(displayModelName('claude-sonnet-4-6[1m]')).toBe('Sonnet 4.6 (1M)');
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

  it('maps CC short aliases (and their 1M variants) to their picker labels', () => {
    expect(displayModelName('opus')).toBe('Opus (latest)');
    expect(displayModelName('opus[1m]')).toBe('Opus (latest, 1M)');
    expect(displayModelName('sonnet')).toBe('Sonnet (latest)');
    expect(displayModelName('sonnet[1m]')).toBe('Sonnet 4.6 (1M)');
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

  describe('names the effort in force, not a pick', () => {
    const pick = { kind: 'device', device_id: 'test-device' };
    const capture = (effort: string, extra: Record<string, unknown> = {}) => ({
      type: 'ContextCaptured',
      channel: 'claude_code',
      model: 'claude-opus-5-6',
      reasoning_effort: effort,
      usage: { input_tokens: 1000, output_tokens: 10 },
      ...extra,
    }) as unknown as StoredEvent;

    it('reads the newest main-session capture over a later pick', () => {
      const events = eventsMap([
        [1, { type: 'MessageReceived', text: 'fix', channel: 'claude_code', created: '2026-01-01T00:00:01Z' }],
        [2, { type: 'CodingAgentSettingsChanged', model: 'claude-opus-5-6', reasoning_effort: 'medium', created: '2026-01-01T00:00:02Z' }],
        [3, capture('medium')],
        [4, { type: 'CodingAgentSettingsChanged', reasoning_effort: 'xhigh', actor: pick, created: '2026-01-01T00:00:04Z' } as unknown as StoredEvent],
        [5, capture('medium')],
      ]);
      expect(exchangeReasoningEffort(groupIntoExchanges(events)[0])).toBe('medium');
    });

    it('skips a pick before the first call, keeping the spawn setting', () => {
      const events = eventsMap([
        [1, { type: 'MessageReceived', text: 'fix', channel: 'claude_code', created: '2026-01-01T00:00:01Z' }],
        [2, { type: 'CodingAgentSettingsChanged', reasoning_effort: 'medium', created: '2026-01-01T00:00:02Z' }],
        [3, { type: 'CodingAgentSettingsChanged', reasoning_effort: 'xhigh', actor: pick, created: '2026-01-01T00:00:03Z' } as unknown as StoredEvent],
      ]);
      expect(exchangeReasoningEffort(groupIntoExchanges(events)[0])).toBe('medium');
    });

    it('ignores a sub-agent call', () => {
      const events = eventsMap([
        [1, { type: 'MessageReceived', text: 'fix', channel: 'claude_code', created: '2026-01-01T00:00:01Z' }],
        [2, capture('medium')],
        [3, capture('low', { parent_tool_use_id: 'toolu_1' })],
      ]);
      expect(exchangeReasoningEffort(groupIntoExchanges(events)[0])).toBe('medium');
    });
  });
});

describe('a rung pick between coding-agent turns', () => {
  const pick = { kind: 'device', device_id: 'test-device' };
  const events = eventsMap([
    [1, { type: 'MessageReceived', text: 'fix', channel: 'claude_code', created: '2026-01-01T00:00:01Z' }],
    [2, { type: 'CodingAgentSettingsChanged', model: 'claude-opus-5-6', reasoning_effort: 'medium', created: '2026-01-01T00:00:02Z' }],
    [3, { type: 'CodingAgentIdled', has_changes: false, created: '2026-01-01T00:00:03Z' }],
    [4, { type: 'CodingAgentSettingsChanged', model: 'claude-sonnet-5-6', actor: pick, created: '2026-01-01T00:00:04Z' } as unknown as StoredEvent],
    [5, { type: 'CodingAgentSettingsChanged', reasoning_effort: 'high', actor: pick, created: '2026-01-01T00:00:05Z' } as unknown as StoredEvent],
    [6, {
      type: 'MessageReceived',
      text: 'go on',
      channel: 'claude_code',
      model: 'claude-sonnet-5-6',
      reasoning_effort: 'high',
      created: '2026-01-01T00:00:06Z',
    }],
  ]);

  it('leaves the earlier turn answered by the pair it ran on', () => {
    const [earlier] = groupIntoExchanges(events);
    expect(exchangeResponseModel(earlier)).toBe('claude-opus-5-6');
    expect(exchangeReasoningEffort(earlier)).toBe('medium');
  });

  it('answers the next turn by its starter pair, before any spawn settings land', () => {
    const [, next] = groupIntoExchanges(events);
    expect(exchangeResponseModel(next)).toBe('claude-sonnet-5-6');
    expect(exchangeReasoningEffort(next)).toBe('high');
  });
});

describe('starter-event fallback on a trigger exchange', () => {
  const FIRING: [number, StoredEvent][] = [
    [1, {
      type: 'TriggerStarted',
      trigger_id: 'trg-1',
      trigger_name: 'Email triage',
      model: 'claude-opus-5[1m]',
      reasoning_effort: 'high',
      created: '2026-01-01T00:00:01Z',
    }],
    [2, { type: 'ToolCalled', name: 'load_knowhow', args: {}, created: '2026-01-01T00:00:02Z' }],
    [3, { type: 'ToolResult', name: 'load_knowhow', result: 'ok', created: '2026-01-01T00:00:03Z' }],
  ];

  it('shows the fire\'s model and effort while the response is still streaming', () => {
    const exchanges = groupIntoExchanges(eventsMap(FIRING));
    expect(exchanges).toHaveLength(1);
    expect(exchangeResponseModel(exchanges[0])).toBe('claude-opus-5[1m]');
    expect(exchangeReasoningEffort(exchanges[0])).toBe('high');
  });

  it('prefers ResponseGenerated once the turn ends', () => {
    const exchanges = groupIntoExchanges(eventsMap([
      ...FIRING,
      [4, {
        type: 'ResponseGenerated',
        text: 'triaged',
        model: 'claude-sonnet-5',
        reasoning_effort: 'medium',
        created: '2026-01-01T00:00:04Z',
      }],
    ]));
    expect(exchangeResponseModel(exchanges[0])).toBe('claude-sonnet-5');
    expect(exchangeReasoningEffort(exchanges[0])).toBe('medium');
  });

  it('stays undefined on a legacy TriggerStarted that recorded neither', () => {
    const exchanges = groupIntoExchanges(eventsMap([
      [1, { type: 'TriggerStarted', trigger_id: 'trg-1', created: '2026-01-01T00:00:01Z' }],
      [2, { type: 'ToolCalled', name: 'load_knowhow', args: {}, created: '2026-01-01T00:00:02Z' }],
    ]));
    expect(exchangeResponseModel(exchanges[0])).toBeUndefined();
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
