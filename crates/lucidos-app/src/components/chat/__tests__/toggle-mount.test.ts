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
    focusedThreadId.value = 'thread-abc';
    expect(computeToggleVisibility(focusedThreadId.value, false, 'active').togglesMounted).toBe(false);

    focusedThreadId.value = null;
    expect(computeToggleVisibility(focusedThreadId.value, false).togglesMounted).toBe(true);
  });

  it('stays mounted when focused thread is in composing state (regression)', () => {
    const { togglesMounted, togglesFading } = computeToggleVisibility('thread-fresh', false, 'composing');
    expect(togglesMounted).toBe(true);
    expect(togglesFading).toBe(false);
  });

  it('hides on the terminal state (discarded)', () => {
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
    inputMode.value = { type: 'do' };
    expect(effectiveSendMode(makeComposingThread('claude_code'))).toBe('claude_code');

    inputMode.value = { type: 'coding_agent' };
    expect(effectiveSendMode(makeComposingThread('lucidos'))).toBe('lucidos');
  });

  it("falls back to currentComposeMode if a composing thread's composeMode is missing", () => {
    inputMode.value = { type: 'coding_agent' };
    expect(effectiveSendMode(makeComposingThread(null))).toBe('claude_code');
  });

  it('routes via composeMode even when the draft was started in another channel (regression)', () => {
    expect(effectiveSendMode(makeComposingThread('claude_code', 'chat'))).toBe('claude_code');
  });

  it('uses channel for active threads (channel is locked once active)', () => {
    const cc = { meta: { id: 'cc-active', state: 'active' as const, channel: 'claude_code' as const } };
    const chat = { meta: { id: 'chat-active', state: 'active' as const, channel: 'chat' as const } };
    expect(effectiveSendMode(cc)).toBe('claude_code');
    expect(effectiveSendMode(chat)).toBe('lucidos');
  });
});
