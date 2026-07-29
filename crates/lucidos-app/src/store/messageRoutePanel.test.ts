import { describe, it, expect, beforeEach } from 'vitest';
import { messageRoutePanel, toggleMessageRoutePanel, closeMessageRoutePanel, type MessageRoutePanelState } from './store';
import type { Exchange } from './thread-events';

function makeState(overrides: Partial<MessageRoutePanelState> & { userSeq?: number } = {}): MessageRoutePanelState {
  const { userSeq = 1, exchange, ...rest } = overrides;
  return {
    anchor: {} as HTMLElement,
    exchange: exchange ?? ({ userSeq, steps: [], userEvent: { type: 'MessageReceived' } } as unknown as Exchange),
    threadId: 't1',
    section: 'origin',
    ...rest,
  };
}

describe('toggleMessageRoutePanel', () => {
  beforeEach(() => closeMessageRoutePanel());

  it('opens the panel when none is open', () => {
    const state = makeState();
    toggleMessageRoutePanel(state);
    expect(messageRoutePanel.value).toBe(state);
  });

  it('closes the panel when the same exchange and section are clicked again', () => {
    const anchor = {} as HTMLElement;
    toggleMessageRoutePanel(makeState({ userSeq: 7, section: 'origin', anchor }));
    toggleMessageRoutePanel(makeState({ userSeq: 7, section: 'origin', anchor }));
    expect(messageRoutePanel.value).toBeNull();
  });

  it('still closes when the badge DOM node was replaced between clicks (streaming re-render)', () => {
    const a1 = {} as HTMLElement;
    const a2 = {} as HTMLElement;
    expect(a1).not.toBe(a2);
    toggleMessageRoutePanel(makeState({ userSeq: 7, section: 'origin', anchor: a1 }));
    toggleMessageRoutePanel(makeState({ userSeq: 7, section: 'origin', anchor: a2 }));
    expect(messageRoutePanel.value).toBeNull();
  });

  it('switches to a different exchange without closing', () => {
    toggleMessageRoutePanel(makeState({ userSeq: 1 }));
    toggleMessageRoutePanel(makeState({ userSeq: 2 }));
    expect(messageRoutePanel.value?.exchange.userSeq).toBe(2);
  });

  it('switches sections when the same exchange is reused with a different section', () => {
    toggleMessageRoutePanel(makeState({ userSeq: 1, section: 'origin' }));
    toggleMessageRoutePanel(makeState({ userSeq: 1, section: 'executor' }));
    expect(messageRoutePanel.value?.section).toBe('executor');
  });

  it('switches across threads even when userSeq + section match', () => {
    toggleMessageRoutePanel(makeState({ threadId: 'a', userSeq: 1 }));
    toggleMessageRoutePanel(makeState({ threadId: 'b', userSeq: 1 }));
    expect(messageRoutePanel.value?.threadId).toBe('b');
  });
});
