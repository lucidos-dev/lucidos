/**
 * A busy change row says what its action is doing. An apply names its phase,
 * so the reader sees that a long wait is hardening rather than a stuck merge.
 */
import { describe, it, expect } from 'vitest';
import { rowProgressLabel } from '../ChangesView';
import type { Change } from '../../../api/client';
import type { ApplyAllBatch, ApplyPhaseReading } from '../../../store/store';

function makeChange(over: Partial<Change> = {}): Change {
  return {
    id: 'c-1',
    request_id: 'req-1',
    thread_id: 'thread-A',
    thread_title: null,
    branch_name: 'b',
    repo_root: '/r',
    description: 'feat: a change',
    file_count: 1,
    files: ['a.ts'],
    requires_restart: false,
    hardened: true,
    status: 'pending',
    created_at: '2026-01-01T00:00:00Z',
    resolved_at: null,
    pre_merge_sha: null,
    post_merge_sha: null,
    commits: [],
    summary: null,
    incomplete: false,
    ...over,
  };
}

const NO_PHASES = new Map<string, ApplyPhaseReading>();

function phases(phase: ApplyPhaseReading['phase']): Map<string, ApplyPhaseReading> {
  return new Map([['thread-A', { phase, eventId: 'e-1', startedAt: null }]]);
}

function batch(over: Partial<ApplyAllBatch> = {}): ApplyAllBatch {
  return { changeIds: ['c-2', 'c-1'], resolvedChangeIds: [], applyingChangeIds: [], resolvingChangeIds: [], ...over };
}

describe('a single apply', () => {
  it('says it is hardening once the engine says so', () => {
    expect(rowProgressLabel(makeChange(), 'apply', phases('hardening'), null)).toBe('Hardening...');
  });

  it('says it is hardening for an unhardened change before any event lands', () => {
    expect(rowProgressLabel(makeChange({ hardened: false }), 'apply', NO_PHASES, null)).toBe('Hardening...');
  });

  it('goes back to applying once hardening hands over to the merge', () => {
    expect(rowProgressLabel(makeChange({ hardened: false }), 'apply', phases('merging'), null)).toBe('Applying...');
  });

  it('says it is resolving a conflict from the served flag alone', () => {
    expect(rowProgressLabel(makeChange({ resolving_conflict: true }), undefined, NO_PHASES, null))
      .toBe('Resolving...');
  });
});

// A live batch never fills `applyingChangeIds`: only a reload serves it. So
// the cases below run with the lists empty, as an open page has them.
describe('an Apply All member', () => {
  it('reads as applying while nothing places it, never as hardening ahead of its turn', () => {
    expect(rowProgressLabel(makeChange({ hardened: false }), undefined, NO_PHASES, batch())).toBe('Applying...');
  });

  it('names the phase an event said, once the batch reaches it', () => {
    expect(rowProgressLabel(makeChange(), undefined, phases('hardening'), batch())).toBe('Hardening...');
  });

  it('names a conflict it is parked on', () => {
    expect(rowProgressLabel(makeChange({ resolving_conflict: true }), undefined, NO_PHASES, batch())).toBe('Resolving...');
  });

  it('names its hardening after a reload, from what the engine served', () => {
    const served = batch({ applyingChangeIds: ['c-1'] });
    expect(rowProgressLabel(makeChange({ hardened: false }), undefined, NO_PHASES, served)).toBe('Hardening...');
  });
});

describe('a row action that is not an apply', () => {
  it('says it is discarding, never applying', () => {
    expect(rowProgressLabel(makeChange(), 'discard', NO_PHASES, null)).toBe('Discarding...');
  });

  it('says it is setting the change aside', () => {
    expect(rowProgressLabel(makeChange(), 'set-aside', NO_PHASES, null)).toBe('Setting aside...');
  });
});
