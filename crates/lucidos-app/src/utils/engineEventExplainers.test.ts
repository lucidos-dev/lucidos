import { describe, it, expect } from 'vitest';
import { describeEngineReason } from './engineEventExplainers';

describe('describeEngineReason', () => {
  it('returns explainer for session_recovered', () => {
    expect(describeEngineReason({ kind: 'session_recovered' }))
      .toMatch(/auto-resumed/i);
  });
  it('returns explainer for orphan_recovery', () => {
    expect(describeEngineReason({ kind: 'orphan_recovery' }))
      .toMatch(/orphaned/i);
  });
  it('returns explainer for harden_retrigger', () => {
    expect(describeEngineReason({ kind: 'harden_retrigger' }))
      .toMatch(/harden/i);
  });
  it('returns explainer for stale_session', () => {
    expect(describeEngineReason({ kind: 'stale_session' }))
      .toMatch(/stale/i);
  });
  it('returns explainer for merge_conflict', () => {
    expect(describeEngineReason({ kind: 'merge_conflict' }))
      .toMatch(/conflict/i);
  });
  it('returns explainer for missing_hardening', () => {
    expect(describeEngineReason({ kind: 'missing_hardening' }))
      .toMatch(/harden/i);
  });
  it('returns null for scheduler (handled by trigger renderer)', () => {
    expect(describeEngineReason({ kind: 'scheduler', trigger_id: 't' }))
      .toBeNull();
  });
});
