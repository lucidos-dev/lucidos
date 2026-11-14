/** With the Cache cards switch off, the transcript draws no keep-alive card.
 *  The render window clamps by row index, so it must count the same rows the
 *  exchange draws, or the clamp cuts the wrong rows. */
import { describe, expect, it } from 'vitest';
import type { Exchange, StoredEvent } from '../thread-events';
import { drawnResponseEvents, exchangeResponseEvents } from './exchange-render';

const at = (minute: number) => `2026-10-03T14:0${minute}:00.000Z`;

const status = {
  type: 'CacheKeepAliveStatusChanged', created: at(2),
  status: { state: 'warming', since: at(1), beats: 1, budget_beats: 12, beat_every_secs: 230, cost_usd: 0.05, context_tokens: 100_000, give_up_at: at(9) },
} as unknown as StoredEvent;

const reply = { type: 'ResponseGenerated', text: 'done', created: at(3), request_event_id: 'm1' } as StoredEvent;

const turn: Exchange = {
  userEvent: { type: 'MessageReceived', text: 'go', mode: 'human', created: at(0), _eventId: 'm1' } as StoredEvent,
  userSeq: 1,
  steps: [{ seq: 2, event: status }, { seq: 3, event: reply }],
};

const kinds = (cacheCards: boolean) => drawnResponseEvents(turn, false, true, cacheCards).map((e) => e.type);

describe('the rows the transcript draws', () => {
  it('keep the keep-alive card while the switch is on', () => {
    expect(kinds(true)).toEqual(exchangeResponseEvents(turn, false, true).map((e) => e.type));
    expect(kinds(true)).toContain('cache_keep_alive');
  });

  it('drop it while the switch is off, and nothing else', () => {
    expect(kinds(false)).toEqual(kinds(true).filter((k) => k !== 'cache_keep_alive'));
  });
});
