import { describe, it, expect } from 'vitest';
import { groupIntoExchanges, exchangeKey, type StoredEvent } from '../thread-events';


let seq = 0;
let clock = 0;

function ev(type: string, extra: Record<string, unknown> = {}): [number, StoredEvent] {
  seq += 1;
  clock += 1;
  return [seq, {
    type,
    created: new Date(Date.UTC(2026, 0, 1, 0, 0, clock)).toISOString(),
    _eventId: `evt-${seq}`,
    ...extra,
  } as unknown as StoredEvent];
}

function turn(text: string): Array<[number, StoredEvent]> {
  return [
    ev('MessageReceived', { content: text }),
    ev('ToolCalled', { name: 'bash', description: `run for ${text}` }),
    ev('ToolResult', { name: 'bash', tool_called_event_id: `evt-${seq}` }),
    ev('TextStreamed', { text: `answer to ${text}` }),
    ev('ResponseGenerated', { content: `answer to ${text}` }),
  ];
}

describe('the tail folds the same, paged or whole', () => {
  it('draws the same turns for the tail it renders', () => {
    seq = 0;
    clock = 0;
    const older = [...turn('one'), ...turn('two')];
    const newer = [...turn('three'), ...turn('four')];

    const whole = groupIntoExchanges(new Map([...older, ...newer]));
    const paged = groupIntoExchanges(new Map(newer));

    expect(paged.map(exchangeKey)).toEqual(whole.slice(2).map(exchangeKey));
    expect(paged.map(e => e.steps.length)).toEqual(whole.slice(2).map(e => e.steps.length));
  });

  it('folds by the clock, not by the order a backfill inserted rows', () => {
    seq = 0;
    clock = 0;
    const older = [...turn('one'), ...turn('two')];
    const newer = [...turn('three'), ...turn('four')];

    const whole = groupIntoExchanges(new Map([...older, ...newer]));
    const hostile = groupIntoExchanges(new Map([...newer, ...older]));

    expect(hostile.map(exchangeKey)).toEqual(whole.map(exchangeKey));
    expect(hostile.map(e => e.steps.length)).toEqual(whole.map(e => e.steps.length));
  });

  it('does not split a turn whose steps all landed in the page', () => {
    seq = 0;
    clock = 0;
    const one = turn('only');
    const paged = groupIntoExchanges(new Map(one));

    expect(paged).toHaveLength(1);
    expect(paged[0].steps.length).toBeGreaterThan(0);
  });
});
