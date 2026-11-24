import { describe, it, expect, beforeAll } from 'vitest';

localStorage.setItem('lucidos-repo-selected-change-id', 'change-saved-from-prior-session');

beforeAll(async () => {
  await import('../effects');
});

describe('persistence effect cold-start', () => {
  it('does not wipe repoSelectedChangeId saved by a prior session', () => {
    expect(localStorage.getItem('lucidos-repo-selected-change-id')).toBe('change-saved-from-prior-session');
  });
});
