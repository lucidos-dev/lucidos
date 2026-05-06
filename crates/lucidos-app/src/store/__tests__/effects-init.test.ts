import { describe, it, expect, beforeAll } from 'vitest';

// Must run BEFORE the store/effects modules load — they read localStorage at
// import time, and the persistence effect for repoSelectedChangeId fires on
// initial subscription. If the signal were initialized to null, that first
// fire would wipe the saved key before useStartup could restore it.
localStorage.setItem('lucidos-repo-selected-change-id', 'change-saved-from-prior-session');

beforeAll(async () => {
  await import('../effects');
});

describe('persistence effect cold-start', () => {
  it('does not wipe repoSelectedChangeId saved by a prior session', () => {
    expect(localStorage.getItem('lucidos-repo-selected-change-id')).toBe('change-saved-from-prior-session');
  });
});
