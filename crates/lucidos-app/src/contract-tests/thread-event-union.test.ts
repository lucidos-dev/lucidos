
import { describe, it, expect } from 'vitest';
import { EVENT_CLASSIFICATION } from '../generated/thread-lifecycle';
import { THREAD_EVENT_TYPE_NAMES } from '../store/thread-events';

describe('ThreadEvent union covers the generated EVENT_CLASSIFICATION', () => {
  it('every classified event type has a matching ThreadEvent union member', () => {
    const missing = Object.keys(EVENT_CLASSIFICATION).filter(
      (name) => !THREAD_EVENT_TYPE_NAMES.has(name as never),
    );
    expect(
      missing,
      `These event types are in the generated EVENT_CLASSIFICATION but missing ` +
        `from the generated ThreadEvent union. Regenerate both: ./scripts/codegen.sh. ` +
        `Missing: ${missing.join(', ')}`,
    ).toEqual([]);
  });

  it('includes WorktreeCleaned (the originally-drifted variant)', () => {
    expect(THREAD_EVENT_TYPE_NAMES.has('WorktreeCleaned')).toBe(true);
    expect(EVENT_CLASSIFICATION.WorktreeCleaned).toBe('metadata');
  });

  it('keeps the retired members the classification map omits', () => {
    for (const retired of ['ContextTokensMeasured', 'ContextAssembled', 'MemorySearched']) {
      expect(
        THREAD_EVENT_TYPE_NAMES.has(retired as never),
        `${retired} is read by exchange-render.ts for old DB rows. It belongs in ` +
          `LEGACY_VARIANTS in crates/lucidos-wire-codegen/src/lib.rs.`,
      ).toBe(true);
      expect(EVENT_CLASSIFICATION[retired]).toBeUndefined();
    }
  });
});
