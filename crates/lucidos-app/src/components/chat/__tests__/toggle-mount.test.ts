/**
 * Tests the compose destination picker's mount logic (the row that picks
 * Lucidos Agent vs a coding target).
 *
 * Bug: togglesMounted was an independent useState synced via useEffect.
 * When focusedThreadId went from non-null to null, there was a render
 * where showToggles=true but togglesMounted=false (effect hadn't fired).
 *
 * Fix: derive togglesMounted = showToggles || fading. The toggle is
 * always mounted when showToggles is true — no effect delay.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { focusedThreadId, inputMode } from '../../../store/store';
import type { ThreadComposeState, ComposeChannelMode, ThreadMeta } from '../../../store/thread-events';
import { effectiveSendMode } from '../promptToggleMode';
import { _resetComposeDraftsForTesting, setDraft } from '../../../store/composeDrafts';

function computeToggleVisibility(
  focusedId: string | null,
  fading: boolean,
  focusedComposeState: ThreadComposeState | null = null,
) {
  // Toggle is for picking the channel of the user's NEXT send. That choice is
  // mutable while the thread is `composing`; once it transitions to `active`
  // (first send) the channel is locked, so the toggle no longer applies.
  const showToggles = !focusedId || focusedComposeState === 'composing';
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

  it('unmounted after fade completes on active focused thread', () => {
    expect(computeToggleVisibility('thread-1', false, 'active').togglesMounted).toBe(false);
  });

  it('stays mounted during fade-out', () => {
    const { togglesMounted, togglesFading } = computeToggleVisibility('thread-1', true, 'active');
    expect(togglesMounted).toBe(true);
    expect(togglesFading).toBe(true);
  });

  it('mounts immediately when returning to compose (no effect delay)', () => {
    // With the old code, togglesMounted was false here until useEffect fired.
    // With derived logic, showToggles=true makes togglesMounted immediately true.
    focusedThreadId.value = 'thread-abc';
    expect(computeToggleVisibility(focusedThreadId.value, false, 'active').togglesMounted).toBe(false);

    focusedThreadId.value = null;
    expect(computeToggleVisibility(focusedThreadId.value, false).togglesMounted).toBe(true);
  });

  it('stays mounted when focused thread is in composing state (regression)', () => {
    // Bug: a freshly-POSTed thread is focused (focusedThreadId is set) but
    // still in `composing` state — the user hasn't sent yet and must be able
    // to re-pick the compose destination (Lucidos Agent vs a coding target).
    // Hiding the picker stranded them on whichever mode was last active in
    // the global inputMode signal.
    const { togglesMounted, togglesFading } = computeToggleVisibility('thread-fresh', false, 'composing');
    expect(togglesMounted).toBe(true);
    expect(togglesFading).toBe(false);
  });

  it('hides on the terminal state (discarded)', () => {
    // Archived threads are NOT a separate compose-state value any more — they
    // carry state='active' plus archive_state='archived' (orthogonal axes).
    // Toggle visibility on archived rows is gated by the archive flag a
    // layer up; this assertion only covers the compose-side terminal.
    expect(computeToggleVisibility('t1', false, 'discarded').togglesMounted).toBe(false);
  });
});

let nextId = 0;
function makeComposingThread(composeMode: ComposeChannelMode, channel: ThreadMeta['channel'] = 'chat') {
  const id = `tcm-${++nextId}`;
  setDraft(id, { text: '', image_hashes: [], mode: composeMode });
  return { meta: { id, state: 'composing' as const, channel } };
}

describe('effectiveSendMode', () => {
  beforeEach(() => {
    inputMode.value = { type: 'do' };
    _resetComposeDraftsForTesting();
    nextId = 0;
  });

  it('falls back to currentComposeMode when no thread is focused (compose view)', () => {
    inputMode.value = { type: 'coding_agent' };
    expect(effectiveSendMode(undefined)).toBe('claude_code');
    inputMode.value = { type: 'do' };
    expect(effectiveSendMode(undefined)).toBe('lucidos');
  });

  it("reflects the focused composing thread's composeMode, not inputMode (regression)", () => {
    // A draft created in Claude mode used to render with the Lucidos toggle
    // active if the user later switched the global inputMode in another tab,
    // or focused a different draft with a different mode. The toggle showed
    // global preference, masking the actual draft channel that would be used
    // on send. Fix: derive from the thread's composeMode while composing.
    inputMode.value = { type: 'do' };
    expect(effectiveSendMode(makeComposingThread('claude_code'))).toBe('claude_code');

    inputMode.value = { type: 'coding_agent' };
    expect(effectiveSendMode(makeComposingThread('lucidos'))).toBe('lucidos');
  });

  it("falls back to currentComposeMode if a composing thread's composeMode is missing", () => {
    // composeMode can be null when the row arrived via SSE before the engine
    // ack'd with the picked mode.
    inputMode.value = { type: 'coding_agent' };
    expect(effectiveSendMode(makeComposingThread(null))).toBe('claude_code');
  });

  it('routes via composeMode even when the draft was started in another channel (regression)', () => {
    // A draft created on the Lucidos Agent (channel='chat',
    // composeMode='lucidos') gets re-pointed at a coding target.
    // applyDestination updates composeMode to 'claude_code' but never touches
    // channel — channel only locks at send time. The submit path used to read
    // channel here, so the picker showed a coding destination while the
    // message routed via Lucidos. effectiveSendMode is the single source.
    expect(effectiveSendMode(makeComposingThread('claude_code', 'chat'))).toBe('claude_code');
  });

  it('uses channel for active threads (channel is locked once active)', () => {
    const cc = { meta: { id: 'cc-active', state: 'active' as const, channel: 'claude_code' as const } };
    const chat = { meta: { id: 'chat-active', state: 'active' as const, channel: 'chat' as const } };
    expect(effectiveSendMode(cc)).toBe('claude_code');
    expect(effectiveSendMode(chat)).toBe('lucidos');
  });
});
