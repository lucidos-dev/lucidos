/** The call shortcut presses what the composer's call toggle presses.
 *
 *  It asks the toggle's own spec for its gate, so it cannot press a hidden
 *  toggle. The follow shortcut is pinned beside its button, in
 *  `follow-toggle.test.tsx`.
 *
 *  Plan: `docs/plans/2026-10-01-shortcuts-for-every-toggle.md`. */
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('../../../store/voice', () => ({ pressCallToggle: vi.fn() }));

import { pressCallToggleIfShown } from '../PromptRowControls';
import { pressCallToggle } from '../../../store/voice';
import { focusedThreadId, preferences, threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';

function focus(thread: ReturnType<typeof makeThreadState> | null): void {
  threadMap.value = thread ? new Map([[thread.meta.id, thread]]) : new Map();
  focusedThreadId.value = thread?.meta.id ?? null;
}

beforeEach(() => {
  vi.clearAllMocks();
  preferences.value = { status: 'loaded', data: { voice_enabled: 'true' } };
});

describe('the call shortcut', () => {
  it('presses the call toggle on the home thread', () => {
    focus(makeThreadState('t1', { meta: { home: true } }));
    pressCallToggleIfShown();
    expect(pressCallToggle).toHaveBeenCalledTimes(1);
  });

  /** Voice sessions live in the home thread alone (ADR 0362). */
  it('does nothing on any other Lucidos Agent thread', () => {
    focus(makeThreadState('t1'));
    pressCallToggleIfShown();
    expect(pressCallToggle).not.toHaveBeenCalled();
  });

  it('does nothing on a coding-agent thread, where the toggle is not drawn', () => {
    focus(makeThreadState('t1', { meta: { channel: 'claude_code', home: true } }));
    pressCallToggleIfShown();
    expect(pressCallToggle).not.toHaveBeenCalled();
  });

  it('does nothing while voice is off', () => {
    preferences.value = { status: 'loaded', data: {} };
    focus(makeThreadState('t1', { meta: { home: true } }));
    pressCallToggleIfShown();
    expect(pressCallToggle).not.toHaveBeenCalled();
  });
});
