import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { ThreadState } from '../thread-events';

/**
 * An event deep link names the event, and the transcript draws only turns and
 * failure cards. So `landOnEvent` hands the scroll layer the turn holding a
 * step. The reported case was a "needs login" notification on a mid-turn
 * `CredentialRequested`, which toasted "That event is not shown in this thread".
 */

const scrollToEventAndPulse = vi.hoisted(() => vi.fn());
vi.mock('../../components/chat/scrollState', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../components/chat/scrollState')>()),
  scrollToEventAndPulse,
}));

const { threadMap } = await import('../store');
const { landOnEvent } = await import('./threads');

function threadWith(events: [string, string][], eventsLoaded = true): ThreadState {
  const map = new Map<number, unknown>();
  let seq = 0;
  for (const [type, id] of events) {
    map.set(++seq, { type, _eventId: id, content: 'go', created: '2026-01-01T10:00:00Z' });
  }
  return { events: map, pendingUserMessages: [], eventsLoaded } as unknown as ThreadState;
}

/** The anchor `landOnEvent` asked the scroll layer to look for, right now. */
function anchorNow(): string | null | undefined {
  const opts = scrollToEventAndPulse.mock.lastCall?.[1];
  return opts?.anchorFor?.();
}

describe('landOnEvent', () => {
  beforeEach(() => {
    scrollToEventAndPulse.mockReset();
    threadMap.value = new Map();
  });

  it('lands a mid-turn form request on the turn holding it', () => {
    threadMap.value = new Map([['t-1', threadWith([
      ['MessageReceived', 'start-1'],
      ['ToolCalled', 'tool-1'],
      ['CredentialRequested', 'cred-1'],
    ])]]);

    landOnEvent('t-1', 'cred-1');

    expect(scrollToEventAndPulse).toHaveBeenCalledWith('cred-1', expect.anything());
    expect(anchorNow()).toBe('start-1');
  });

  it('keeps an event that draws itself as its own target', () => {
    threadMap.value = new Map([['t-1', threadWith([['MessageReceived', 'start-1']])]]);
    landOnEvent('t-1', 'start-1');
    expect(anchorNow()).toBe('start-1');
  });

  // A cold tap: the thread arrives after the link started.
  it('finds the anchor once the thread arrives', () => {
    landOnEvent('t-1', 'cred-1');
    expect(anchorNow()).toBeNull();

    threadMap.value = new Map([['t-1', threadWith([
      ['MessageReceived', 'start-1'],
      ['CredentialRequested', 'cred-1'],
    ])]]);
    expect(anchorNow()).toBe('start-1');
  });
});
