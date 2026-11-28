import { describe, expect, it, vi } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { SideQuestionIcon } from '../../shared/icons';
import type { HeaderActionSpec } from '../../layout/headerActions';
import {
  codingAgentMenuAction, composerRowMembers, promptRowToggles, sideQuestionToggleAction,
} from '../PromptRowControls';
import { CODING_AGENT_MENU_FOLD_KEY } from '../ControlMenuTrigger';
import { RUNG_FOLD_KEY } from '../RungButtons';
import { callIsOffered } from '../../../voice/callOffer';
import type { ThreadState } from '../../../store/thread-events/thread-meta';

interface AnyVNode extends VNode<{ children?: ComponentChildren; [k: string]: unknown }> {}

const spec = (key: string): HeaderActionSpec => ({ key, label: key, icon: () => null });

const members = (codingAgent: boolean, hasText = true) => composerRowMembers({
  codingAgentMenu: codingAgent ? codingAgentMenuAction('claude-code') : null,
  attachImage: spec('attach-image'),
  readouts: [spec('todo-indicator'), spec('waiting')],
  wipPreview: spec('wip'),
  clearDraft: hasText ? spec('prompt-clear') : null,
  toggles: [spec('call-toggle'), spec('side-question')],
  situational: [spec('thread-diff'), spec('standing-apply')],
  rung: spec(RUNG_FOLD_KEY),
});

const order = (codingAgent: boolean) => {
  const { row, cluster } = members(codingAgent);
  return [...row, ...cluster].map((a) => a.key);
};

const foldOrder = (codingAgent: boolean) => members(codingAgent).foldOrder.map((a) => a.key);

describe('the composer row order', () => {
  it('runs from the toggles to the model picker, as arranged', () => {
    expect(order(true)).toEqual([
      'call-toggle', 'side-question', CODING_AGENT_MENU_FOLD_KEY,
      'attach-image', 'todo-indicator', 'waiting', 'wip', 'prompt-clear',
      'thread-diff', 'standing-apply', RUNG_FOLD_KEY,
    ]);
  });

  it('folds todo and waiting first, then the rest as arranged', () => {
    expect(foldOrder(true)).toEqual([
      'todo-indicator', 'waiting',
      'call-toggle', 'side-question', CODING_AGENT_MENU_FOLD_KEY,
      'attach-image', 'wip', 'prompt-clear',
      'thread-diff', 'standing-apply', RUNG_FOLD_KEY,
    ]);
  });

  it('folds todo and waiting before the toggles', () => {
    for (const codingAgent of [false, true]) {
      const keys = foldOrder(codingAgent);
      for (const readout of ['todo-indicator', 'waiting']) {
        expect(keys.indexOf(readout)).toBeLessThan(keys.indexOf('call-toggle'));
        expect(keys.indexOf(readout)).toBeLessThan(keys.indexOf('side-question'));
      }
    }
  });

  it('folds Diff and the standing apply before the model picker', () => {
    const keys = foldOrder(true);
    expect(keys.indexOf('thread-diff')).toBeLessThan(keys.indexOf(RUNG_FOLD_KEY));
    expect(keys.indexOf('standing-apply')).toBeLessThan(keys.indexOf(RUNG_FOLD_KEY));
  });

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
    expect(order(true).slice(0, 2)).toEqual(order(false).slice(0, 2));
  });

  it('offers the coding-agent menu to a coding-agent thread alone', () => {
    expect(order(false)[0]).toBe('call-toggle');
    expect(order(false)).not.toContain(CODING_AGENT_MENU_FOLD_KEY);
  });

  it('offers no coding-agent menu to Codex', () => {
    expect(codingAgentMenuAction('codex')).toBeNull();
  });
});

describe('the Diff and apply merge point', () => {
  it('is the leading toggle while one stands', () => {
    expect(members(true).mergeAt).toBe('call-toggle');
  });

  it('falls to the next member when the row has no toggles', () => {
    const at = (codingAgent: boolean) => composerRowMembers({
      codingAgentMenu: codingAgent ? codingAgentMenuAction('claude-code') : null,
      attachImage: spec('attach-image'),
      readouts: [spec('todo-indicator')],
      wipPreview: null,
      clearDraft: null,
      toggles: [],
      situational: [spec('thread-diff'), spec('standing-apply')],
      rung: spec(RUNG_FOLD_KEY),
    }).mergeAt;
    expect(at(true)).toBe(CODING_AGENT_MENU_FOLD_KEY);
    expect(at(false)).toBe('attach-image');
  });
});

describe('the row toggles', () => {
  const sideQuestion = sideQuestionToggleAction(false, () => {});

  it('hold the side-question toggle alone with no call', () => {
    expect(promptRowToggles(null, undefined, sideQuestion).map((a) => a.key)).toEqual([sideQuestion.key]);
  });

  it('are empty where neither a call nor a side question is offered', () => {
    expect(promptRowToggles(null, undefined, null)).toEqual([]);
  });

  it('offer no call member on a coding-agent thread', () => {
    expect(promptRowToggles('claude-code', undefined, null).map((a) => a.key))
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

  it('offers a call on the home thread with the Lucidos Agent', () => {
    expect(callIsOffered(thread(true), null)).toBe(true);
  });

  it('offers none on another thread, in the compose view, or to a coding agent', () => {
    expect(callIsOffered(thread(false), null)).toBe(false);
    expect(callIsOffered(undefined, null)).toBe(false);
    expect(callIsOffered(thread(true), 'claude-code')).toBe(false);
  });
});
