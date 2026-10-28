/** The follow toggle floats over the composer's top edge, in the down
 *  chevron's slot, and its shortcut presses what it presses.
 *
 *  Both ask one resolver for the state: the follow seed in compose context and
 *  the live flag everywhere else. So the shortcut cannot light what the button
 *  would not.
 *
 *  Plan: `docs/plans/2026-10-28-the-follow-toggle-floats-with-the-down-chevron.md`. */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';

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

import { FollowLiveEdgeToggle, toggleFollowLiveEdge } from '../FollowToggle';
import { followingLiveEdge, followLiveEdgeSeed, setFollowLiveEdge } from '../scrollState';
import { FollowLiveEdgeIcon } from '../../shared/icons';
import { ToggleIconButton } from '../../shared/ToggleIconButton';
import { keepFocusOnPress } from '../../../utils/dom';
import { focusedThreadId, threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import type { Signal } from '@preact/signals';

interface AnyVNode extends VNode<{ children?: ComponentChildren; [k: string]: unknown }> {}

const live = followingLiveEdge as Signal<boolean>;
const seed = followLiveEdgeSeed as Signal<boolean>;

function focus(thread: ReturnType<typeof makeThreadState> | null): void {
  threadMap.value = thread ? new Map([[thread.meta.id, thread]]) : new Map();
  focusedThreadId.value = thread?.meta.id ?? null;
}

const button = () => FollowLiveEdgeToggle() as AnyVNode;
const icon = () => button().props.children as AnyVNode;

beforeEach(() => {
  vi.clearAllMocks();
  live.value = false;
  seed.value = false;
});

describe('the floating follow toggle', () => {
  it('is a glyph-tone toggle named for the e2e helpers and the slot rules', () => {
    focus(makeThreadState('t1'));
    const b = button();
    expect(b.type).toBe(ToggleIconButton);
    expect(b.props.tone).toBe('glyph');
    expect(b.props['data-role']).toBe('follow-live-edge');
    expect(b.props.class).toBe('follow-toggle');
    expect(icon().type).toBe(FollowLiveEdgeIcon);
  });

  it('shows the live flag over a thread, never the seed', () => {
    focus(makeThreadState('t1'));
    seed.value = true;
    expect(button().props.pressed).toBe(false);
    live.value = true;
    expect(button().props.pressed).toBe(true);
  });

  it('shows the seed in the compose view and on a composing draft', () => {
    for (const thread of [null, makeThreadState('draft', { meta: { state: 'composing' } })]) {
      focus(thread);
      live.value = true;
      seed.value = false;
      expect(button().props.pressed).toBe(false);
      seed.value = true;
      expect(button().props.pressed).toBe(true);
    }
  });

  it('drives the fill, the label, the tooltip and the press off that one state', () => {
    focus(makeThreadState('t1'));
    for (const on of [false, true]) {
      live.value = on;
      const b = button();
      expect(b.props.pressed).toBe(on);
      expect(icon().props.armed).toBe(on);
      expect(b.props['aria-label']).toBe(on ? 'Stop following the live edge' : 'Follow the live edge');
      expect(String(b.props['data-tooltip'])).toMatch(on ? /^Following the live edge/ : /^Follow the live edge:/);
      (b.props.onClick as () => void)();
      expect(setFollowLiveEdge).toHaveBeenLastCalledWith(!on);
    }
  });

  it('keeps the keyboard up when pressed', () => {
    // The reader arms and disarms it while typing. A press that blurred the
    // prompt dropped the iOS keyboard mid-sentence.
    focus(makeThreadState('t1'));
    expect(button().props.onMouseDown).toBe(keepFocusOnPress);
  });

  /** A theme without a hue sets `--accent` to the text colour (Mono does), so
   *  an accent-only toggle read the same armed or not. So the magnet fills. */
  it('shows its state by fill, not by colour alone', () => {
    const fillOf = (armed: boolean) => (FollowLiveEdgeIcon({ armed }) as AnyVNode).props.fill;
    expect(fillOf(true)).toBe('currentColor');
    expect(fillOf(false)).toBe('none');
  });
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
