import { describe, it, expect } from 'vitest';
import {
  statusLabel,
  isActive,
  ACTIVE_STATUSES,
} from './exchange-status';

// ===========================================================================
// isActive
// ===========================================================================
describe('isActive', () => {
  it('pending is active', () => expect(isActive('pending')).toBe(true));
  it('streaming is active', () => expect(isActive('streaming')).toBe(true));
  it('coding-agent-working is active', () => expect(isActive('coding-agent-working')).toBe(true));
  it('done is NOT active', () => expect(isActive('done')).toBe(false));
  it('interrupted is NOT active', () => expect(isActive('interrupted')).toBe(false));
  it('canceled is NOT active', () => expect(isActive('canceled')).toBe(false));
  it('error is NOT active', () => expect(isActive('error')).toBe(false));
  it('aborted is NOT active', () => expect(isActive('aborted')).toBe(false));
  it('awaiting-answer is NOT active (spinner stops while user thinks)', () =>
    expect(isActive('awaiting-answer')).toBe(false));
});

describe('ACTIVE_STATUSES', () => {
  it('contains exactly pending, streaming, coding-agent-working', () => {
    expect(ACTIVE_STATUSES).toEqual(new Set(['pending', 'streaming', 'coding-agent-working']));
  });
});

// ===========================================================================
// statusLabel
// ===========================================================================
describe('statusLabel', () => {
  it('pending without steps → Requesting', () => {
    const result = statusLabel('pending', false);
    expect(result.label).toBe('Requesting');
    expect(result.className).toBe('working');
  });

  it('pending with steps → Working', () => {
    const result = statusLabel('pending', true);
    expect(result.label).toBe('Working');
    expect(result.className).toBe('working');
  });

  it('streaming without steps → Requesting', () => {
    const result = statusLabel('streaming', false);
    expect(result.label).toBe('Requesting');
    expect(result.className).toBe('working');
  });

  it('streaming with steps → Working', () => {
    const result = statusLabel('streaming', true);
    expect(result.label).toBe('Working');
    expect(result.className).toBe('working');
  });

  it('coding-agent-working → Working', () => {
    const result = statusLabel('coding-agent-working', false);
    expect(result.label).toBe('Working');
    expect(result.className).toBe('working');
  });

  it('done → Done', () => {
    const result = statusLabel('done', false);
    expect(result.label).toBe('Done');
    expect(result.className).toBe('done');
  });

  it('interrupted → Done', () => {
    const result = statusLabel('interrupted', false);
    expect(result.label).toBe('Done');
    expect(result.className).toBe('done');
  });

  it('queued → Queued', () => {
    const result = statusLabel('queued', false);
    expect(result.label).toBe('Queued');
    expect(result.className).toBe('queued');
  });

  it('canceled → Canceled', () => {
    const result = statusLabel('canceled', false);
    expect(result.label).toBe('Canceled');
    expect(result.className).toBe('canceled');
  });

  it('error → Error', () => {
    const result = statusLabel('error', false);
    expect(result.label).toBe('Error');
    expect(result.className).toBe('error');
  });

  it('aborted → Aborted (distinct from canceled)', () => {
    const result = statusLabel('aborted', false);
    expect(result.label).toBe('Aborted');
    expect(result.className).toBe('aborted');
  });

  it('awaiting-answer → Needs your answer (distinct from Done)', () => {
    const result = statusLabel('awaiting-answer', false);
    expect(result.label).toBe('Needs your answer');
    expect(result.className).toBe('awaiting');
  });
});
