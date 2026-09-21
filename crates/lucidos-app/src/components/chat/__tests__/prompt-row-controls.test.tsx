/** The prompt row's three leading slots, and the order that is their contract.
 *
 *  The control menu anchors the row, the follow toggle is second and the call
 *  toggle third, and each keeps its slot on every thread. The follow toggle
 *  once sat after the indicators. It was third on a Lucidos thread, second on a
 *  coding-agent one and fourth with a subscription armed. It is the only
 *  control here rendering in every state, so it is the one that must not move.
 *
 *  The two toggles DO fold, last of everything, which is the same rule seen
 *  from the other end. A slot holds at every width that can show it, and a
 *  glyph off the screen protects nothing.
 *
 *  Plan: `docs/plans/2026-09-19-the-composer-row-is-one-row.md`. */
import { describe, expect, it } from 'vitest';
import { Fragment } from 'preact';
import type { ComponentChildren, VNode } from 'preact';
import { FollowLiveEdgeIcon } from '../../shared/icons';
import { CodingAgentControlMenu } from '../CodingAgentControlMenu';
import { LucidosControlMenu } from '../LucidosControlMenu';
import { PromptRowControls, promptRowToggles } from '../PromptRowControls';

interface AnyVNode extends VNode<{ children?: ComponentChildren; [k: string]: unknown }> {}

/** The cluster's rendered children, by component, in document order.
 *
 *  Walks the returned tree rather than mounting it: these are function
 *  components, so an unmounted vnode carries the component itself as `type`,
 *  which is exactly the identity the assertions are about. Nothing here needs a
 *  DOM, a store or a signal.
 *
 *  `Fragment` is skipped rather than reported, and that is the point of the
 *  walk: a fragment emits no DOM, so the cluster's children ARE the row's
 *  children whichever branch they came wrapped in.
 */
function componentsIn(node: ComponentChildren): unknown[] {
  if (node === null || node === undefined || typeof node === 'boolean') return [];
  if (typeof node === 'string' || typeof node === 'number') return [];
  if (Array.isArray(node)) return node.flatMap(componentsIn);
  const vnode = node as AnyVNode;
  const self = typeof vnode.type === 'function' && vnode.type !== Fragment ? [vnode.type] : [];
  return [...self, ...componentsIn(vnode.props?.children)];
}

const row = (codingAgent: 'claude-code' | 'codex' | null) =>
  PromptRowControls({
    codingAgent,
    codingAgentThreadId: codingAgent ? 'thread-1' : undefined,
    composeThreadId: undefined,
    lucidosThreadId: codingAgent ? undefined : 'thread-1',
    composeContext: false,
    // Nothing folded, which is the state the order contract is about.
    toggles: promptRowToggles(codingAgent, false).row,
    attrsFor: () => ({}),
  });

const cluster = (codingAgent: 'claude-code' | 'codex' | null) => componentsIn(row(codingAgent));

describe('PromptRowControls', () => {
  /** The branch exists for the controls that genuinely belong to one backend.
   *  A coding-agent thread has no Lucidos model picker to offer. */
  it('keeps the Lucidos menu off a coding-agent thread', () => {
    const codingAgent = cluster('claude-code');
    expect(codingAgent).toContain(CodingAgentControlMenu);
    expect(codingAgent).not.toContain(LucidosControlMenu);
  });

  it('keeps the coding-agent menu off a Lucidos Agent thread', () => {
    const lucidos = cluster(null);
    expect(lucidos).toContain(LucidosControlMenu);
    expect(lucidos).not.toContain(CodingAgentControlMenu);
  });

  /** `.prompt-actions-row` is a flex row whose children are diffed
   *  positionally, so the order is part of the contract.
   *
   *  The follow toggle shows up here as its ICON, the one function component
   *  inside the `<button>` this walk can see. The call toggle is its own
   *  component, so it shows up under its own name. */
  it('puts the menu first and the follow toggle second', () => {
    // Voice ships off, so the call toggle draws nothing here and contributes no
    // member. Its THIRD slot is the contract, asserted on the fold order below.
    expect(cluster(null)).toEqual([LucidosControlMenu, FollowLiveEdgeIcon]);
    expect(cluster('claude-code')).toEqual([CodingAgentControlMenu, FollowLiveEdgeIcon]);
  });

});

describe('the two fixed toggles', () => {
  /** Two views of one pair, so the fold and the row cannot disagree about which
   *  of them exists. */
  it('are the same members in both orders', () => {
    const { row: slots, fold } = promptRowToggles(null, false);
    expect([...slots].sort()).toEqual([...fold].sort());
  });

  /** The follow toggle is the only control rendering in every state, so it is
   *  the LAST thing to leave the row. */
  it('fold call first and follow last', () => {
    const { fold } = promptRowToggles(null, false);
    expect(fold[fold.length - 1].key).toBe('follow-live-edge');
  });

  /** A call reaches the Lucidos Agent and nothing else (ADR 0165). Voice ships
   *  off, so the member is absent either way here. What this pins is that the
   *  factory is asked, rather than the composer deciding for itself. */
  it('offer no call member on a coding-agent thread', () => {
    expect(promptRowToggles('claude-code', false).fold.map((a) => a.key))
      .not.toContain('call-toggle');
  });
});
