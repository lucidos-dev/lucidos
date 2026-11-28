import { describe, it, expect, beforeEach } from 'vitest';
import { applyAllBatch, applyingChangeIds, applyNowRequests, busyChangeIds, changes } from '../store';

beforeEach(() => {
  applyingChangeIds.value = new Set();
  applyNowRequests.value = new Map();
  changes.value = { status: 'loaded', data: [] };
  applyAllBatch.value = null;
});

describe('busyChangeIds', () => {
  it('holds every unresolved member of a restored batch', () => {
    applyAllBatch.value = { changeIds: ['c-1', 'c-2', 'c-3'], resolvedChangeIds: ['c-1'], applyingChangeIds: [], resolvingChangeIds: [] };
    expect([...busyChangeIds.value].sort()).toEqual(['c-2', 'c-3']);
  });

  it('lets a member go once it resolves', () => {
    applyAllBatch.value = { changeIds: ['c-1'], resolvedChangeIds: ['c-1'], applyingChangeIds: [], resolvingChangeIds: [] };
    expect(busyChangeIds.value.size).toBe(0);
  });
});
