import { describe, it, expect } from 'vitest';
import { TS, makeThreadState } from './thread-events-helpers';
import { exchangeResponseEvents, groupIntoExchanges, type StoredEvent, type ThreadEvent } from '../thread-events';

/** Replay events into one thread and return its exchanges' response rows. */
function rows(events: ThreadEvent[]) {
  const thread = makeThreadState();
  events.forEach((event, i) => {
    thread.events.set(i + 1, { ...event, created: TS, _eventId: `e${i + 1}` } as unknown as StoredEvent);
  });
  const exchanges = groupIntoExchanges(thread.events);
  return { exchanges, rows: exchanges.flatMap((x) => exchangeResponseEvents(x)) };
}

const MESSAGE = { type: 'MessageReceived', text: 'compare the fares' } as unknown as ThreadEvent;
const SHOWN = { type: 'WidgetShown', app_id: 'fare-grid' } as unknown as ThreadEvent;
const PINNED = { type: 'WidgetPinned', app_id: 'fare-grid' } as unknown as ThreadEvent;
const UNPINNED = { type: 'WidgetUnpinned', app_id: 'fare-grid' } as unknown as ThreadEvent;
// Legacy rows: the snapshot serves the pre-rename names raw.
const RESTORED = { type: 'WidgetRestored', app_id: 'fare-grid' } as unknown as ThreadEvent;
const HIDDEN = { type: 'WidgetHidden', app_id: 'fare-grid' } as unknown as ThreadEvent;

describe('a widget in the transcript (ADR 0402)', () => {
  it('draws at the turn that showed it, carrying its event id', () => {
    const { rows: drawn } = rows([MESSAGE, SHOWN]);
    expect(drawn.filter((r) => r.type === 'widget')).toEqual([
      { type: 'widget', app_id: 'fare-grid', event_id: 'e2', seq: 2 },
    ]);
  });

  it('stays drawn after a pin and an unpin, which add no turn and no row', () => {
    const before = rows([MESSAGE, SHOWN]);
    const after = rows([MESSAGE, SHOWN, PINNED, UNPINNED]);
    expect(after.exchanges).toHaveLength(before.exchanges.length);
    expect(after.rows.filter((r) => r.type === 'widget')).toHaveLength(1);
  });

  it('reads the legacy names the same way', () => {
    const before = rows([MESSAGE, SHOWN]);
    const after = rows([MESSAGE, SHOWN, RESTORED, HIDDEN]);
    expect(after.exchanges).toHaveLength(before.exchanges.length);
    expect(after.rows).toEqual(before.rows);
  });
});
