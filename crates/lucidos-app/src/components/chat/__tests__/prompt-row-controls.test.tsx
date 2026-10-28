/** The composer row's order and its fold order.
 *
 *  The fold takes a prefix of the fold order. That follows the row left to
 *  right, except that todo and waiting fold first: the toggles keep their slots.
 *  The user asked for the Claude menu and attach image to fold before the model
 *  picker. The WIP preview toggle keeps its place in the order.
 *
 *  Plan: `docs/plans/2026-10-21-coding-agent-menu-folds-before-the-model-picker.md`. */
import { describe, expect, it, vi } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { FollowLiveEdgeIcon, SideQuestionIcon } from '../../shared/icons';
import type { HeaderActionSpec } from '../../layout/headerActions';
import {
  codingAgentMenuAction, composerRowMembers, followLiveEdgeAction, promptRowToggles, sideQuestionToggleAction,
} from '../PromptRowControls';
import { CODING_AGENT_MENU_FOLD_KEY } from '../ControlMenuTrigger';
import { RUNG_FOLD_KEY } from '../RungButtons';
import { callIsOffered } from '../../../voice/callOffer';
import type { ThreadState } from '../../../store/thread-events/thread-meta';

interface AnyVNode extends VNode<{ children?: ComponentChildren; [k: string]: unknown }> {}

const spec = (key: string): HeaderActionSpec => ({ key, label: key, icon: () => null });

/** Every member present, as on a busy coding-agent thread. */
const members = (codingAgent: boolean, hasText = true) => composerRowMembers({
  codingAgentMenu: codingAgent ? codingAgentMenuAction('claude-code') : null,
  attachImage: spec('attach-image'),
  readouts: [spec('todo-indicator'), spec('waiting')],
  wipPreview: spec('wip'),
  clearDraft: hasText ? spec('prompt-clear') : null,
  toggles: [spec('call-toggle'), spec('follow-live-edge'), spec('side-question')],
  situational: [spec('thread-diff'), spec('standing-apply')],
  rung: spec(RUNG_FOLD_KEY),
});

/** Left to right, as the row draws them. */
const order = (codingAgent: boolean) => {
  const { row, cluster } = members(codingAgent);
  return [...row, ...cluster].map((a) => a.key);
};

/** The order the fold takes them in. */
const foldOrder = (codingAgent: boolean) => members(codingAgent).foldOrder.map((a) => a.key);

describe('the composer row order', () => {
  it('runs from the toggles to the model picker, as arranged', () => {
    expect(order(true)).toEqual([
      'call-toggle', 'follow-live-edge', 'side-question', CODING_AGENT_MENU_FOLD_KEY,
      'attach-image', 'todo-indicator', 'waiting', 'wip', 'prompt-clear',
      'thread-diff', 'standing-apply', RUNG_FOLD_KEY,
    ]);
  });

  it('folds todo and waiting first, then the rest as arranged', () => {
    expect(foldOrder(true)).toEqual([
      'todo-indicator', 'waiting',
      'call-toggle', 'follow-live-edge', 'side-question', CODING_AGENT_MENU_FOLD_KEY,
      'attach-image', 'wip', 'prompt-clear',
      'thread-diff', 'standing-apply', RUNG_FOLD_KEY,
    ]);
  });

  it('folds todo and waiting before the follow and side-question toggles', () => {
    for (const codingAgent of [false, true]) {
      const keys = foldOrder(codingAgent);
      for (const readout of ['todo-indicator', 'waiting']) {
        expect(keys.indexOf(readout)).toBeLessThan(keys.indexOf('follow-live-edge'));
        expect(keys.indexOf(readout)).toBeLessThan(keys.indexOf('side-question'));
      }
    }
  });

  /** The picker holds its spot: what comes and goes stands left of it, so it
   *  folds first and never pushes the picker. */
  it('folds Diff and the standing apply before the model picker', () => {
    const keys = foldOrder(true);
    expect(keys.indexOf('thread-diff')).toBeLessThan(keys.indexOf(RUNG_FOLD_KEY));
    expect(keys.indexOf('standing-apply')).toBeLessThan(keys.indexOf(RUNG_FOLD_KEY));
  });

  /** It reads no thread state, so this holds in compose context and in a
   *  started thread alike. */
  it('folds the Claude menu and attach image before the model picker', () => {
    const keys = foldOrder(true);
    expect(keys.indexOf(CODING_AGENT_MENU_FOLD_KEY)).toBeLessThan(keys.indexOf(RUNG_FOLD_KEY));
    expect(keys.indexOf('attach-image')).toBeLessThan(keys.indexOf(RUNG_FOLD_KEY));
  });

  it('keeps the other icons in their slots when clear draft appears or disappears', () => {
    for (const codingAgent of [false, true]) {
      const empty = members(codingAgent, false);
      const typed = members(codingAgent, true);
      expect(typed.row.map((a) => a.key)).toEqual([
        ...empty.row.map((a) => a.key), 'prompt-clear',
      ]);
      expect(typed.cluster.map((a) => a.key)).toEqual(empty.cluster.map((a) => a.key));
    }
  });

  it('renders the situational members and the model picker in the right-hand cluster', () => {
    expect(members(true).cluster.map((a) => a.key)).toEqual(['thread-diff', 'standing-apply', RUNG_FOLD_KEY]);
  });

  it('leads with the same toggles on a coding-agent thread as on a Lucidos Agent one', () => {
    expect(order(true).slice(0, 3)).toEqual(order(false).slice(0, 3));
  });

  /** A Lucidos Agent thread has no slash commands, and its model pick is the
   *  rung control's. */
  it('offers the coding-agent menu to a coding-agent thread alone', () => {
    expect(order(false)[0]).toBe('call-toggle');
    expect(order(false)).not.toContain(CODING_AGENT_MENU_FOLD_KEY);
  });

  /** Codex has no slash commands, so its menu could never open. The row drew
   *  it anyway, dimmed for good. */
  it('offers no coding-agent menu to Codex', () => {
    expect(codingAgentMenuAction('codex')).toBeNull();
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
  const sideQuestion = sideQuestionToggleAction(false, () => {});

  /** The follow toggle renders in every state, and the side-question toggle
   *  stands right after it. */
  it('put the follow toggle first with no call, and the side-question toggle after it', () => {
    expect(promptRowToggles(null, false, undefined, sideQuestion).map((a) => a.key))
      .toEqual(['follow-live-edge', sideQuestion.key]);
  });

  it('close on the follow toggle where no side question can be asked', () => {
    const toggles = promptRowToggles(null, false, undefined, null);
    expect(toggles.map((a) => a.key)).toEqual(['follow-live-edge']);
  });

  /** A call reaches the Lucidos Agent and nothing else (ADR 0165). Voice ships
   *  off, so the member is absent either way here. What this pins is that the
   *  factory is asked, rather than the composer deciding for itself. */
  it('offer no call member on a coding-agent thread', () => {
    expect(promptRowToggles('claude-code', false, undefined, null).map((a) => a.key))
      .not.toContain('call-toggle');
  });
});

describe('the side-question toggle', () => {
  it('lights while the mode is on, and names the way back', () => {
    const off = sideQuestionToggleAction(false, () => {});
    const on = sideQuestionToggleAction(true, () => {});
    expect([off.active, on.active]).toEqual([false, true]);
    expect([off.label, on.label]).toEqual(['Ask a side question', 'Back to a normal message']);
  });

  it('is a glyph that keeps the keyboard up, and presses through the toggle it is given', () => {
    const toggle = vi.fn();
    const action = sideQuestionToggleAction(false, toggle);
    expect(action.tone).toBe('glyph');
    expect(action.keepsFocus).toBe(true);
    action.onClick?.({} as never);
    expect(toggle).toHaveBeenCalledOnce();
  });

  it('draws the circled question mark', () => {
    const icon = sideQuestionToggleAction(false, () => {}).icon() as AnyVNode;
    expect(icon.type).toBe(SideQuestionIcon);
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
