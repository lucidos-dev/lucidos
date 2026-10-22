/** The composer row's order, which is also its fold order.
 *
 *  The fold takes a prefix, so the leftmost member folds first. The user asked
 *  for the Claude menu and attach image to fold before the model picker. They
 *  also asked that a fold follow the order the row is arranged in.
 *
 *  Plan: `docs/plans/2026-10-21-coding-agent-menu-folds-before-the-model-picker.md`. */
import { describe, expect, it } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { FollowLiveEdgeIcon } from '../../shared/icons';
import type { HeaderActionSpec } from '../../layout/headerActions';
import {
  codingAgentMenuAction, composerRowMembers, followLiveEdgeAction, promptRowToggles,
} from '../PromptRowControls';
import { CODING_AGENT_MENU_FOLD_KEY } from '../ControlMenuTrigger';
import { RUNG_FOLD_KEY } from '../RungButtons';
import { callIsOffered } from '../../../voice/callOffer';
import type { ThreadState } from '../../../store/thread-events/thread-meta';

interface AnyVNode extends VNode<{ children?: ComponentChildren; [k: string]: unknown }> {}

const spec = (key: string): HeaderActionSpec => ({ key, label: key, icon: () => null });

/** Every member present, as on a busy coding-agent thread. */
const members = (codingAgent: boolean) => composerRowMembers({
  codingAgentMenu: codingAgent ? codingAgentMenuAction('claude-code') : null,
  attachImage: spec('attach-image'),
  readouts: [spec('waiting'), spec('wip')],
  clearDraft: spec('prompt-clear'),
  toggles: [spec('call-toggle'), spec('follow-live-edge')],
  rung: spec(RUNG_FOLD_KEY),
  banner: [spec('diff'), spec('apply')],
});

/** Left to right, which is the order the fold takes them in. */
const order = (codingAgent: boolean) => {
  const { row, cluster } = members(codingAgent);
  return [...row, ...cluster].map((a) => a.key);
};

describe('the composer row order', () => {
  it('runs from the Claude menu to the banner, as arranged', () => {
    expect(order(true)).toEqual([
      CODING_AGENT_MENU_FOLD_KEY, 'attach-image', 'waiting', 'wip', 'prompt-clear',
      'call-toggle', 'follow-live-edge', RUNG_FOLD_KEY, 'diff', 'apply',
    ]);
  });

  /** It reads no thread state, so this holds in compose context and in a
   *  started thread alike. */
  it('folds the Claude menu and attach image before the model picker', () => {
    const keys = order(true);
    expect(keys.indexOf(CODING_AGENT_MENU_FOLD_KEY)).toBeLessThan(keys.indexOf(RUNG_FOLD_KEY));
    expect(keys.indexOf('attach-image')).toBeLessThan(keys.indexOf(RUNG_FOLD_KEY));
  });

  it('puts the follow toggle right before the model picker', () => {
    const keys = order(true);
    expect(keys.indexOf(RUNG_FOLD_KEY) - keys.indexOf('follow-live-edge')).toBe(1);
  });

  it('renders the model picker and the banner in the right-hand cluster', () => {
    expect(members(true).cluster.map((a) => a.key)).toEqual([RUNG_FOLD_KEY, 'diff', 'apply']);
  });

  /** A Lucidos Agent thread has no slash commands, and its model pick is the
   *  rung control's. */
  it('offers the coding-agent menu to a coding-agent thread alone', () => {
    expect(order(false)[0]).toBe('attach-image');
    expect(order(false)).not.toContain(CODING_AGENT_MENU_FOLD_KEY);
  });
});

/** The follow toggle's on-state must not rest on colour alone. A theme without
 *  a hue sets `--accent` to the text colour (Mono does), and there an
 *  accent-only toggle read the same armed or not. So the magnet fills. */
describe('the follow toggle shows its state by shape', () => {
  const fillOf = (armed: boolean) =>
    (FollowLiveEdgeIcon({ armed }) as AnyVNode).props.fill;

  it('fills the magnet when armed and leaves it open when not', () => {
    expect(fillOf(true)).toBe('currentColor');
    expect(fillOf(false)).toBe('none');
  });

  it('hands the icon the same state the button claims', () => {
    for (const composeContext of [false, true]) {
      const action = followLiveEdgeAction(composeContext);
      const icon = action.icon() as AnyVNode;
      expect(icon.props.armed).toBe(action.active);
    }
  });
});

describe('the row toggles', () => {
  /** The follow toggle renders in every state, so it closes the row's left
   *  side, right before the model picker. */
  it('end with the follow toggle', () => {
    const toggles = promptRowToggles(null, false, undefined);
    expect(toggles[toggles.length - 1].key).toBe('follow-live-edge');
  });

  /** A call reaches the Lucidos Agent and nothing else (ADR 0165). Voice ships
   *  off, so the member is absent either way here. What this pins is that the
   *  factory is asked, rather than the composer deciding for itself. */
  it('offer no call member on a coding-agent thread', () => {
    expect(promptRowToggles('claude-code', false, undefined).map((a) => a.key))
      .not.toContain('call-toggle');
  });
});

describe('the call toggle is offered on the home thread alone', () => {
  const thread = (home: boolean) => ({ meta: home ? { home: true } : {} }) as unknown as ThreadState;

  /** Voice sessions live in the home thread (ADR 0362). Any other thread, and
   *  the compose view with no thread at all, offer no call. */
  it('offers a call on the home thread with the Lucidos Agent', () => {
    expect(callIsOffered(thread(true), null)).toBe(true);
  });

  it('offers none on another thread, in the compose view, or to a coding agent', () => {
    expect(callIsOffered(thread(false), null)).toBe(false);
    expect(callIsOffered(undefined, null)).toBe(false);
    expect(callIsOffered(thread(true), 'claude-code')).toBe(false);
  });
});
