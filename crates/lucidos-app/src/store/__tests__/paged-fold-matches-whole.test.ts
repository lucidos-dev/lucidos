import { describe, it, expect } from 'vitest';
import { groupIntoExchanges, exchangeKey, type StoredEvent } from '../thread-events';

/**
 * Paging changes WHAT IS LOADED, never how it groups.
 *
 * The transcript folds whatever the client holds. A paged open holds a tail, so
 * the turns it draws must be the turns a whole load would have drawn there. A
 * fold that regroups at the page boundary is the failure: a step under the
 * wrong turn, or one turn split into two.
 *
 * The plan is
 * docs/plans/2026-09-20-a-long-thread-opens-without-its-whole-history.md.
 */

let seq = 0;
let clock = 0;

/** One event, stamped on a rising clock so the fold's sort is unambiguous. */
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

/** One ordinary turn: the user speaks, the agent works, the agent answers. */
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
    // The page: only the newest rows, which is what a cold open now holds.
    const paged = groupIntoExchanges(new Map(newer));

    // The tail is the last two turns either way, by identity and by shape.
    expect(paged.map(exchangeKey)).toEqual(whole.slice(2).map(exchangeKey));
    expect(paged.map(e => e.steps.length)).toEqual(whole.slice(2).map(e => e.steps.length));
  });

  it('folds by the clock, not by the order a backfill inserted rows', () => {
    // A page arrives AFTER the rows it belongs in front of, so the merged Map's
    // insertion order is not the transcript's order. The fold sorts, and this is
    // what says so: the hostile order must reach the same reading.
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
    // The shape a boundary could break: one turn's rows are contiguous, so a
    // page containing all of them must yield exactly one exchange.
    seq = 0;
    clock = 0;
    const one = turn('only');
    const paged = groupIntoExchanges(new Map(one));

    expect(paged).toHaveLength(1);
    expect(paged[0].steps.length).toBeGreaterThan(0);
  });
});
