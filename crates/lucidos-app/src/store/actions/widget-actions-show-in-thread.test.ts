import { describe, it, expect, vi, beforeEach } from 'vitest';

/**
 * "Show in thread" lands on the widget's card (ADR 0407). A folded turn draws
 * no body, so the card is not in the DOM until the turn unfolds.
 */

const expandExchange = vi.fn();
const showEventWhereItLives = vi.fn((_eventId: string) => Promise.resolve());
const thread = { id: 'thread-1' };

vi.mock('../store', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../store')>()),
  expandExchange: (threadId: string, userSeq: number) => expandExchange(threadId, userSeq),
  threadMap: { value: new Map([['thread-1', thread]]) },
}));
vi.mock('../thread-events', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../thread-events')>()),
  computeExchanges: () => [
    { userSeq: 3, steps: [{ event: { type: 'ToolCalled', _eventId: 'tool-1' } }] },
    { userSeq: 7, steps: [{ event: { type: 'WidgetShown', _eventId: 'shown-1' } }] },
  ],
}));
vi.mock('./event-navigation', () => ({
  showEventWhereItLives: (id: string) => showEventWhereItLives(id),
}));

const { showWidgetInThread } = await import('./widget-actions');

describe('showWidgetInThread', () => {
  beforeEach(() => {
    expandExchange.mockClear();
    showEventWhereItLives.mockClear();
  });

  it('unfolds the turn that showed the widget, then lands on its card', () => {
    showWidgetInThread('thread-1', 'shown-1');
    expect(expandExchange).toHaveBeenCalledWith('thread-1', 7);
    expect(showEventWhereItLives).toHaveBeenCalledWith('shown-1');
  });

  it('still navigates when no loaded turn holds the event', () => {
    showWidgetInThread('thread-1', 'older-page');
    expect(expandExchange).not.toHaveBeenCalled();
    expect(showEventWhereItLives).toHaveBeenCalledWith('older-page');
  });
});
