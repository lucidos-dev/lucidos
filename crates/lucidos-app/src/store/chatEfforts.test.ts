import { describe, expect, it } from 'vitest';
import { parseChatEfforts, withChatEffort } from './chatEfforts';

describe('parseChatEfforts', () => {
  it('reads each model with its own tier', () => {
    const efforts = parseChatEfforts('claude-opus-5-5[1m]=high, gpt-6.1-sol=low');
    expect(efforts.get('claude-opus-5-5[1m]')).toBe('high');
    expect(efforts.get('gpt-6.1-sol')).toBe('low');
    expect(efforts.get('claude-opus-5-5')).toBeUndefined();
  });

  it('skips a pair the engine would refuse, and keeps the rest', () => {
    const efforts = parseChatEfforts('a=bogus, =low, b, c=medium');
    expect([...efforts]).toEqual([['c', 'medium']]);
  });

  it('reads an unset value as no tiers', () => {
    expect(parseChatEfforts(null).size).toBe(0);
    expect(parseChatEfforts('').size).toBe(0);
  });
});

describe('withChatEffort', () => {
  it('sets one model and keeps every other', () => {
    expect(withChatEffort('a=low, b=high', 'a', 'max')).toBe('a=max, b=high');
    expect(withChatEffort(null, 'a', 'low')).toBe('a=low');
  });

  it('removes one model for null, leaving it at its default effort', () => {
    expect(withChatEffort('a=low, b=high', 'a', null)).toBe('b=high');
    expect(withChatEffort('a=low', 'a', null)).toBe('');
  });
});
