/** The follow and call shortcuts press what the composer's toggles press.
 *
 *  Each one asks the toggle's own spec for its state and its gate. So the
 *  shortcut cannot light what the button would not, or press a hidden toggle.
 *
 *  Plan: `docs/plans/2026-10-01-shortcuts-for-every-toggle.md`. */
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('../scrollState', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../scrollState')>();
  const { signal } = await import('@preact/signals');
  return {
    ...actual,
    followingLiveEdge: signal(false),
    followLiveEdgeSeed: signal(false),
    setFollowLiveEdge: vi.fn(),
  };
});
vi.mock('../../../store/voice', () => ({ pressCallToggle: vi.fn() }));

import { toggleFollowLiveEdge, pressCallToggleIfShown } from '../PromptRowControls';
import { followingLiveEdge, followLiveEdgeSeed, setFollowLiveEdge } from '../scrollState';
import { pressCallToggle } from '../../../store/voice';
import { focusedThreadId, preferences, threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import type { Signal } from '@preact/signals';

const live = followingLiveEdge as Signal<boolean>;
const seed = followLiveEdgeSeed as Signal<boolean>;

function focus(thread: ReturnType<typeof makeThreadState> | null): void {
  threadMap.value = thread ? new Map([[thread.meta.id, thread]]) : new Map();
  focusedThreadId.value = thread?.meta.id ?? null;
}

beforeEach(() => {
  vi.clearAllMocks();
  live.value = false;
  seed.value = false;
  preferences.value = { status: 'loaded', data: { voice_enabled: 'true', home_thread_enabled: 'true' } };
});

describe('the follow shortcut', () => {
  it('arms the follow seed in the compose view, where no transcript exists', () => {
    focus(null);
    toggleFollowLiveEdge();
    expect(setFollowLiveEdge).toHaveBeenCalledWith(true);
  });

  it('disarms a lit seed in the compose view', () => {
    focus(null);
    seed.value = true;
    toggleFollowLiveEdge();
    expect(setFollowLiveEdge).toHaveBeenCalledWith(false);
  });

  it('reads the live follow on an active thread, never the seed', () => {
    focus(makeThreadState('t1'));
    seed.value = true;
    live.value = false;
    toggleFollowLiveEdge();
    expect(setFollowLiveEdge).toHaveBeenCalledWith(true);
  });

  it('reads the seed on a composing draft', () => {
    focus(makeThreadState('draft', { meta: { state: 'composing' } }));
    seed.value = true;
    live.value = false;
    toggleFollowLiveEdge();
    expect(setFollowLiveEdge).toHaveBeenCalledWith(false);
  });
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
