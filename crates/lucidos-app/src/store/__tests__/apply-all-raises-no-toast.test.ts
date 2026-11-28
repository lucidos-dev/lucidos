import { describe, it, expect, beforeAll, beforeEach } from 'vitest';
import { toasts, applyAllInProgress, applyAllBatch, applyAllCanceling, applyPhases } from '../store';

beforeAll(async () => {
  await import('../effects');
});

beforeEach(() => {
  applyAllInProgress.value = false;
  applyAllBatch.value = null;
  applyAllCanceling.value = false;
  applyPhases.value = new Map();
  toasts.value = [];
});

describe('an Apply All in flight', () => {
  it('raises no toast from start to finish', () => {
    applyAllInProgress.value = true;
    applyAllBatch.value = { changeIds: ['c-1', 'c-2'], resolvedChangeIds: [], applyingChangeIds: [], resolvingChangeIds: [] };
    applyPhases.value = new Map([['t-1', { phase: 'hardening', eventId: 'e-1', startedAt: null }]]);
    applyAllBatch.value = { changeIds: ['c-1', 'c-2'], resolvedChangeIds: ['c-1'], applyingChangeIds: [], resolvingChangeIds: [] };
    applyAllCanceling.value = true;
    applyAllInProgress.value = false;
    expect(toasts.value).toEqual([]);
  });
});
