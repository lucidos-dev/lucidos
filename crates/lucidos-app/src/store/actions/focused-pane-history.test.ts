import { describe, it, expect, vi, beforeEach } from 'vitest';

// Stub the two history stacks so the test observes only which one the dispatcher
// routes to — not their (store/DOM-heavy) internals.
vi.mock('./navigation', () => ({ navBack: vi.fn(), navForward: vi.fn() }));
vi.mock('./thread-navigation', () => ({ threadNavBack: vi.fn(), threadNavForward: vi.fn() }));

import { focusedPane } from '../store';
import { navBack, navForward } from './navigation';
import { threadNavBack, threadNavForward } from './thread-navigation';
import { historyBack, historyForward } from './focused-pane-history';

beforeEach(() => {
  vi.mocked(navBack).mockClear();
  vi.mocked(navForward).mockClear();
  vi.mocked(threadNavBack).mockClear();
  vi.mocked(threadNavForward).mockClear();
});

describe('focused-pane-aware history navigation', () => {
  it('routes Back/Forward to the content stack when the content pane is focused', () => {
    focusedPane.value = 'content';
    historyBack();
    historyForward();
    expect(navBack).toHaveBeenCalledTimes(1);
    expect(navForward).toHaveBeenCalledTimes(1);
    expect(threadNavBack).not.toHaveBeenCalled();
    expect(threadNavForward).not.toHaveBeenCalled();
  });

  it('routes Back/Forward to the thread stack when the thread pane is focused', () => {
    focusedPane.value = 'thread';
    historyBack();
    historyForward();
    expect(threadNavBack).toHaveBeenCalledTimes(1);
    expect(threadNavForward).toHaveBeenCalledTimes(1);
    expect(navBack).not.toHaveBeenCalled();
    expect(navForward).not.toHaveBeenCalled();
  });

  it('routes to the thread stack when the drawer is focused (drawer is the thread side)', () => {
    focusedPane.value = 'drawer';
    historyBack();
    historyForward();
    expect(threadNavBack).toHaveBeenCalledTimes(1);
    expect(threadNavForward).toHaveBeenCalledTimes(1);
    expect(navBack).not.toHaveBeenCalled();
    expect(navForward).not.toHaveBeenCalled();
  });
});
