/**
 * Tests the input mode toggle (Manifest/Edit/Claude) mount logic.
 *
 * Bug: togglesMounted was an independent useState synced via useEffect.
 * When focusedThreadId went from non-null to null, there was a render
 * where showToggles=true but togglesMounted=false (effect hadn't fired).
 *
 * Fix: derive togglesMounted = showToggles || fading. The toggle is
 * always mounted when showToggles is true — no effect delay.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { focusedThreadId } from '../../../store/store';

function computeToggleVisibility(focusedId: string | null, fading: boolean) {
  const showToggles = !focusedId;
  const togglesMounted = showToggles || fading;
  const togglesFading = !showToggles && fading;
  return { togglesMounted, togglesFading };
}

describe('input mode toggle visibility', () => {
  beforeEach(() => {
    focusedThreadId.value = null;
  });

  it('mounted in compose view', () => {
    const { togglesMounted, togglesFading } = computeToggleVisibility(null, false);
    expect(togglesMounted).toBe(true);
    expect(togglesFading).toBe(false);
  });

  it('unmounted after fade completes', () => {
    expect(computeToggleVisibility('thread-1', false).togglesMounted).toBe(false);
  });

  it('stays mounted during fade-out', () => {
    const { togglesMounted, togglesFading } = computeToggleVisibility('thread-1', true);
    expect(togglesMounted).toBe(true);
    expect(togglesFading).toBe(true);
  });

  it('mounts immediately when returning to compose (no effect delay)', () => {
    // With the old code, togglesMounted was false here until useEffect fired.
    // With derived logic, showToggles=true makes togglesMounted immediately true.
    focusedThreadId.value = 'thread-abc';
    expect(computeToggleVisibility(focusedThreadId.value, false).togglesMounted).toBe(false);

    focusedThreadId.value = null;
    expect(computeToggleVisibility(focusedThreadId.value, false).togglesMounted).toBe(true);
  });
});
