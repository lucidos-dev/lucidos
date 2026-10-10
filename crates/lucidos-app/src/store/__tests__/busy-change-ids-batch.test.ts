/**
 * An Apply All member stays busy until it resolves, even on a page that never
 * saw the batch start. A reload restores the batch from the server, but not
 * the live `applyingChangeIds`, so the busy set reads the batch itself.
 * Otherwise a queued member offers Apply and Discard mid-batch.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { applyAllBatch, applyingChangeIds, applyingNowThreadIds, busyChangeIds, changes } from '../store';

beforeEach(() => {
  applyingChangeIds.value = new Set();
  applyingNowThreadIds.value = new Map();
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
