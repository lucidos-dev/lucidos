// Drift guard between the TWO generated files: the `EVENT_CLASSIFICATION` map
// in `thread-lifecycle.ts` must be fully covered by the `ThreadEvent` union in
// `thread-event-wire.ts`.
//
// Both come from Rust now, but by different routes, which is why this is still
// a real check. The map is `all_persisted_event_types()` filtered by
// `classify_event`; the union is the `ThreadEvent` enum parsed out of the
// source. `all_persisted_event_types_matches_the_enum` pins those two Rust
// sources together. This pins the two emitted FILES together, so regenerating
// one and not the other is loud.
//
// Direction is one-way by design (EVENT_CLASSIFICATION is a subset of the
// union). The union legitimately carries members the map omits. Those are the
// retired events in the generator's `LEGACY_VARIANTS` table, plus the
// `CommandCheckpointed` / `CommandCheckpointReverted` pair. That pair is
// rendered and persisted, but sits outside the classification surface.

import { describe, it, expect } from 'vitest';
import { EVENT_CLASSIFICATION } from './thread-lifecycle';
import { THREAD_EVENT_TYPE_NAMES } from '../store/thread-events';

describe('ThreadEvent union covers the generated EVENT_CLASSIFICATION', () => {
  it('every classified event type has a matching ThreadEvent union member', () => {
    const missing = Object.keys(EVENT_CLASSIFICATION).filter(
      (name) => !THREAD_EVENT_TYPE_NAMES.has(name as never),
    );
    expect(
      missing,
      `These event types are in the generated EVENT_CLASSIFICATION but missing ` +
        `from the generated ThreadEvent union. Regenerate both: ` +
        `cargo test -p lucidos-engine generate_typescript_file -- --ignored && ` +
        `cargo test -p lucidos-engine generate_thread_event_wire_file -- --ignored. ` +
        `Missing: ${missing.join(', ')}`,
    ).toEqual([]);
  });

  // Anchor: the live drift this guard was built for. WorktreeCleaned was in the
  // generated map and missing from the hand-maintained union.
  it('includes WorktreeCleaned (the originally-drifted variant)', () => {
    expect(THREAD_EVENT_TYPE_NAMES.has('WorktreeCleaned')).toBe(true);
    expect(EVENT_CLASSIFICATION.WorktreeCleaned).toBe('metadata');
  });

  // The retired members are why containment is asserted one way only. They are
  // read by `exchange-render.ts` and must survive every regeneration.
  it('keeps the retired members the classification map omits', () => {
    for (const retired of ['ContextTokensMeasured', 'ContextAssembled', 'MemorySearched']) {
      expect(
        THREAD_EVENT_TYPE_NAMES.has(retired as never),
        `${retired} is read by exchange-render.ts for old DB rows. It belongs in ` +
          `LEGACY_VARIANTS in thread_events_tests/ts_codegen.rs.`,
      ).toBe(true);
      expect(EVENT_CLASSIFICATION[retired]).toBeUndefined();
    }
  });
});
