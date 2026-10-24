import { describe, it, expect } from 'vitest';
import { recencyKey, byRecent, byCreated } from './thread-meta';
import type { ThreadState } from './thread-meta';

/** Minimal ThreadState carrying just the recency fields the sort reads. */
function threadWith(lastUserAction: string | undefined, updatedAt: string): ThreadState {
  return { meta: { lastUserAction, updatedAt } } as unknown as ThreadState;
}

/** Minimal ThreadState carrying just the creation fields `byCreated` reads. */
function threadCreated(createdAt: string, updatedAt = createdAt): ThreadState {
  return { meta: { createdAt, updatedAt } } as unknown as ThreadState;
}

describe('recencyKey', () => {
  it('uses lastUserAction when present', () => {
    expect(recencyKey(threadWith('2026-06-10T00:00:00Z', '2026-06-15T00:00:00Z')))
      .toBe('2026-06-10T00:00:00Z');
  });

  it('falls back to updatedAt when lastUserAction is absent', () => {
    expect(recencyKey(threadWith(undefined, '2026-06-15T00:00:00Z')))
      .toBe('2026-06-15T00:00:00Z');
  });
});

describe('byRecent', () => {
  it('sorts by last user action, NOT last activity (agent churn does not float a thread)', () => {
    // `churned`: agent active 1 minute ago (newer updatedAt) but user acted a day ago.
    // `acted`:   user acted 1 hour ago. `acted` must sort first.
    const churned = threadWith('2026-06-14T00:00:00Z', '2026-06-15T11:59:00Z');
    const acted = threadWith('2026-06-15T11:00:00Z', '2026-06-15T11:00:00Z');
    const sorted = [churned, acted].sort(byRecent);
    expect(sorted[0]).toBe(acted);
    expect(sorted[1]).toBe(churned);
  });
});

describe('byCreated', () => {
  it('sorts by creation time, newest first', () => {
    const older = threadCreated('2026-06-10T00:00:00Z');
    const newer = threadCreated('2026-06-15T00:00:00Z');
    const sorted = [older, newer].sort(byCreated);
    expect(sorted[0]).toBe(newer);
    expect(sorted[1]).toBe(older);
  });

  it('ignores later activity — a freshly-touched old thread keeps its position', () => {
    // `old`: created long ago but updated 1 minute ago. `recent`: created today.
    // `byCreated` must rank by creation, so `recent` still leads.
    const old = threadCreated('2026-01-01T00:00:00Z', '2026-06-15T11:59:00Z');
    const recent = threadCreated('2026-06-15T08:00:00Z');
    const sorted = [old, recent].sort(byCreated);
    expect(sorted[0]).toBe(recent);
    expect(sorted[1]).toBe(old);
  });

  it('falls back to updatedAt for skeleton rows with no createdAt', () => {
    const skeleton = { meta: { createdAt: '', updatedAt: '2026-06-15T00:00:00Z' } } as unknown as ThreadState;
    const older = threadCreated('2026-06-10T00:00:00Z');
    const sorted = [older, skeleton].sort(byCreated);
    expect(sorted[0]).toBe(skeleton);
    expect(sorted[1]).toBe(older);
  });
});
