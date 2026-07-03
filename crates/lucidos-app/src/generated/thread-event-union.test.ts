// Drift guard: the generated EVENT_CLASSIFICATION map (auto-generated from the
// Rust `ThreadEvent` enum via thread_lifecycle.rs) must be fully covered by the
// hand-maintained `ThreadEvent` discriminated union in
// `store/thread-events/thread-event-types.ts`.
//
// Why this exists: a new Rust `ThreadEvent` variant lands automatically in the
// generated `EVENT_CLASSIFICATION` map but NOT in the hand-maintained payload
// union until a human adds it. That drift used to be silent — `WorktreeCleaned`
// shipped in the map but was missing from the union, and a test had to force
// `as unknown as ThreadEvent` to construct it. This test makes the drift loud:
// add a Rust variant, regenerate `thread-lifecycle.ts`, and this fails until the
// matching member is added to the union (and to `THREAD_EVENT_TYPE_FLAGS`, whose
// `satisfies` annotation keeps `THREAD_EVENT_TYPE_NAMES` in lockstep with the
// union at compile time).
//
// Direction is one-way by design (EVENT_CLASSIFICATION ⊆ union). The union
// legitimately carries members absent from the classification map — retired
// legacy events (`ContextTokensMeasured`, `ContextAssembled`) and the
// `CommandCheckpointed` / `CommandCheckpointReverted` pair (rendered + persisted
// but not part of the section/status classification surface) — so the reverse
// containment is not asserted.

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
        `from the ThreadEvent union — add each as a payload member in ` +
        `store/thread-events/thread-event-types.ts (and its key to ` +
        `THREAD_EVENT_TYPE_FLAGS): ${missing.join(', ')}`,
    ).toEqual([]);
  });

  // Anchor: the live drift this guard was built for. WorktreeCleaned is in the
  // generated map and must now be a first-class union member.
  it('includes WorktreeCleaned (the originally-drifted variant)', () => {
    expect(THREAD_EVENT_TYPE_NAMES.has('WorktreeCleaned')).toBe(true);
    expect(EVENT_CLASSIFICATION.WorktreeCleaned).toBe('metadata');
  });
});
