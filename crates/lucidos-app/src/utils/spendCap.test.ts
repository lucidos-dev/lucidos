import { afterEach, describe, expect, it, vi } from 'vitest';
import { preferences } from '../store/store';
import { spendCapResetLabel } from './spendCap';

afterEach(() => {
  vi.useRealTimers();
  preferences.value = { status: 'not-loaded' };
});

describe('spend-cap reset copy', () => {
  it('uses the configured timezone rather than the provider’s UTC clause', () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-11-11T00:30:00Z'));
    preferences.value = { status: 'loaded', data: { timezone: 'Europe/Oslo' } };
    expect(spendCapResetLabel('2026-11-11T01:00:00Z')).toBe('Resets at 02:00.');
    preferences.value = { status: 'loaded', data: { timezone: 'America/Los_Angeles' } };
    expect(spendCapResetLabel('2026-11-11T01:00:00Z')).toBe('Resets at 17:00.');
  });

  it('does not invent a provider reset from the engine’s fallback hold', () => {
    expect(spendCapResetLabel(null)).toBe('Reset time not supplied.');
  });
});
