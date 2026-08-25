import { describe, it, expect } from 'vitest';
import { resolveTriggerScrollStep } from './triggerScrollStep';

const ROWS = [
  { id: 'ungrouped-1' },
  { id: 'in-group', group_id: 'g1' },
];

describe('resolveTriggerScrollStep', () => {
  it('does nothing with no pending target', () => {
    expect(resolveTriggerScrollStep(null, ROWS, new Set())).toEqual({ kind: 'idle' });
  });

  it('scrolls to an ungrouped row', () => {
    expect(resolveTriggerScrollStep('ungrouped-1', ROWS, new Set()))
      .toEqual({ kind: 'scroll', triggerId: 'ungrouped-1' });
  });

  it('scrolls to a grouped row whose group is open', () => {
    expect(resolveTriggerScrollStep('in-group', ROWS, new Set(['other'])))
      .toEqual({ kind: 'scroll', triggerId: 'in-group' });
  });

  it('expands a collapsed group instead of scrolling to a row that is not mounted', () => {
    // The regression this prevents: a collapsed group renders none of its
    // members, so the anchor does not exist. Scrolling would find nothing and
    // the link would silently do nothing, which is the original bug.
    expect(resolveTriggerScrollStep('in-group', ROWS, new Set(['g1'])))
      .toEqual({ kind: 'expand', groupId: 'g1' });
  });

  it('scrolls on the next pass, once the group has been expanded', () => {
    const first = resolveTriggerScrollStep('in-group', ROWS, new Set(['g1']));
    expect(first).toEqual({ kind: 'expand', groupId: 'g1' });
    // The target deliberately survives an expand, so the re-run lands it.
    expect(resolveTriggerScrollStep('in-group', ROWS, new Set()))
      .toEqual({ kind: 'scroll', triggerId: 'in-group' });
  });

  it('drops a target naming no row, rather than holding it', () => {
    // Consume-once: a held stale id would mark an unrelated row later.
    expect(resolveTriggerScrollStep('gone', ROWS, new Set())).toEqual({ kind: 'drop' });
  });
});
