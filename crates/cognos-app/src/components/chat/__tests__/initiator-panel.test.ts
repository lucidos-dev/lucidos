import { describe, it, expect } from 'vitest';
import { describeInitiator } from '../ChatExchange';
import type { Exchange } from '../../../store/thread-events';

function exchangeWith(userEvent: Exchange['userEvent']): Exchange {
  return { userEvent, userSeq: 0, steps: [] };
}

describe('describeInitiator', () => {
  it('user-sent MessageReceived shows "You" with user variant', () => {
    const ex = exchangeWith({
      type: 'MessageReceived',
      text: 'hello',
      sender: 'user',
      channel: 'chat',
    });
    const desc = describeInitiator(ex, '<p>hello</p>', []);
    expect(desc.label).toBe('You');
    expect(desc.variant).toBe('user');
  });

  it('system-injected MessageReceived (parent_thread origin) does NOT show "You"', () => {
    const ex = exchangeWith({
      type: 'MessageReceived',
      text: '[Child thread completed] Thread "X" completed with proposed changes.',
      sender: 'system',
      channel: 'chat',
      origin: { kind: 'parent_thread', thread_id: 'parent-1' },
    });
    const desc = describeInitiator(ex, '<p>...</p>', []);
    expect(desc.label).not.toBe('You');
    expect(desc.variant).toBe('system');
  });

  it('API-originated MessageReceived does NOT show "You"', () => {
    const ex = exchangeWith({
      type: 'MessageReceived',
      text: 'curl request',
      sender: 'user',
      channel: 'chat',
      origin: { kind: 'api', user_agent: 'curl/8.7.1' },
    });
    const desc = describeInitiator(ex, '<p>curl request</p>', []);
    expect(desc.label).not.toBe('You');
    expect(desc.variant).toBe('system');
  });

  // Change lifecycle events surface as their own initiator panels — distinct
  // icons + labels per status, so the timeline auditably shows what happened.
  it('ChangeApplied → "Change applied" with system variant', () => {
    const ex = exchangeWith({ type: 'ChangeApplied', change_id: 'c1' });
    const desc = describeInitiator(ex, '', []);
    expect(desc.label).toBe('Change applied');
    expect(desc.variant).toBe('system');
  });

  it('ChangeDiscarded → "Change discarded"', () => {
    const ex = exchangeWith({ type: 'ChangeDiscarded', change_id: 'c1' });
    const desc = describeInitiator(ex, '', []);
    expect(desc.label).toBe('Change discarded');
    expect(desc.variant).toBe('system');
  });

  it('ChangeReverted → "Change reverted"', () => {
    const ex = exchangeWith({ type: 'ChangeReverted', change_id: 'c1' });
    const desc = describeInitiator(ex, '', []);
    expect(desc.label).toBe('Change reverted');
    expect(desc.variant).toBe('system');
  });

  it('ChangeApplyFailed → "Change failed"', () => {
    const ex = exchangeWith({ type: 'ChangeApplyFailed', change_id: 'c1', error: 'boom' });
    const desc = describeInitiator(ex, '', []);
    expect(desc.label).toBe('Change failed');
    expect(desc.variant).toBe('system');
  });
});
