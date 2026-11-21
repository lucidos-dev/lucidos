/** A card's size is the newest pick for its widget instance. It comes from the
 *  loaded events, or from the server's `cardSizes` for picks behind the page.
 *  I6 of docs/plans/2026-11-03-widgets-one-render-no-pin.md. */
import { afterEach, describe, expect, it } from 'vitest';
import { effect } from '@preact/signals';
import { widgetCardSize } from './widgetCardSizes';
import { threadMap } from './store';
import { bumpThreadEvents, flushThreadEventsBumpsNow } from './threadActivity';
import type { StoredEvent, ThreadState } from './thread-events';
import type { CardSize } from '../api/threads';

function seedThread(events: Array<[number, Partial<StoredEvent>]>, cardSizes?: CardSize[]): void {
  const map = new Map<number, StoredEvent>(events.map(([seq, e]) => [seq, e as StoredEvent]));
  threadMap.value = new Map([['T', { events: map, cardSizes } as unknown as ThreadState]]);
}

afterEach(() => {
  threadMap.value = new Map();
});

describe('widgetCardSize', () => {
  it('reads a pick behind the page from the server card sizes', () => {
    seedThread(
      [[900, { type: 'WidgetShown', app_id: 'player' } as Partial<StoredEvent>]],
      [{ sequence: 10, app_id: 'player', params: { clip: 'a.mp3' }, size: 'large' }],
    );
    expect(widgetCardSize('T', 'player', { clip: 'a.mp3' })).toBe('large');
    expect(widgetCardSize('T', 'player', { clip: 'b.mp3' })).toBeUndefined();
  });

  it('a loaded pick newer than the server one wins', () => {
    seedThread(
      [[900, { type: 'InlineWidgetSized', app_id: 'player', size: 'small' } as Partial<StoredEvent>]],
      [{ sequence: 10, app_id: 'player', size: 'large' }],
    );
    expect(widgetCardSize('T', 'player')).toBe('small');
  });

  it('a server pick newer than every loaded one wins', () => {
    seedThread(
      [[5, { type: 'InlineWidgetSized', app_id: 'player', size: 'small' } as Partial<StoredEvent>]],
      [{ sequence: 10, app_id: 'player', size: 'large' }],
    );
    expect(widgetCardSize('T', 'player')).toBe('large');
  });

  it('follows a live pick, which bumps the thread but not threadMap', () => {
    seedThread([[1, { type: 'WidgetShown', app_id: 'player' } as Partial<StoredEvent>]]);
    const seen: Array<string | undefined> = [];
    const stop = effect(() => { seen.push(widgetCardSize('T', 'player')); });
    const thread = threadMap.value.get('T')!;
    thread.events = new Map(thread.events).set(2, { type: 'InlineWidgetSized', app_id: 'player', size: 'large' } as StoredEvent);
    bumpThreadEvents('T');
    flushThreadEventsBumpsNow();
    stop();
    expect(seen).toEqual([undefined, 'large']);
  });
});
