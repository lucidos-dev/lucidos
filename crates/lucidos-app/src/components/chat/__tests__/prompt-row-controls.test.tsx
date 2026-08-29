/** The prompt row's leading control cluster, and the one thing in it that is
 *  NOT the backend's.
 *
 *  A coding-agent thread arms *event waits* through `lucidos await-event` just
 *  as the Lucidos Agent arms them through the tool, and the whole
 *  `e2e-lock-wait` skill exists to make it do so. The *waiting indicator*
 *  used to be mounted inside the Lucidos arm of this branch, so such a thread
 *  showed the **Waiting** dot with nothing anywhere naming what it watched and
 *  no **Stop waiting** button. These tests pin the mount, from both sides:
 *  the indicator on every branch, the Lucidos-only controls on one. */
import { describe, expect, it } from 'vitest';
import { Fragment } from 'preact';
import type { ComponentChildren, VNode } from 'preact';
import { FollowLiveEdgeIcon } from '../../shared/icons';
import { CallToggle } from '../CallToggle';
import { CodingAgentControlMenu } from '../CodingAgentControlMenu';
import { WaitingIndicator } from '../WaitingPanel';
import { LucidosControlMenu } from '../LucidosControlMenu';
import { PromptRowControls } from '../PromptRowControls';
import { TodoListIndicator } from '../TodoListPanel';

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

const cluster = (codingAgent: 'claude-code' | 'codex' | null) =>
  componentsIn(
    PromptRowControls({
      codingAgent,
      codingAgentThreadId: codingAgent ? 'thread-1' : undefined,
      composeThreadId: undefined,
      lucidosThreadId: codingAgent ? undefined : 'thread-1',
      composeContext: false,
    }),
  );

describe('PromptRowControls', () => {
  it.each<['a Claude Code thread' | 'a Codex thread', 'claude-code' | 'codex']>([
    ['a Claude Code thread', 'claude-code'],
    ['a Codex thread', 'codex'],
  ])('mounts the waiting indicator on %s', (_label, agent) => {
    expect(cluster(agent)).toContain(WaitingIndicator);
  });

  it('mounts the waiting indicator on a Lucidos Agent thread', () => {
    expect(cluster(null)).toContain(WaitingIndicator);
  });

  /** The branch exists for the controls that genuinely belong to one backend.
   *  `TodoListWritten` is the Lucidos Agent's own event, so a coding-agent
   *  thread has no todo list to show and no Lucidos model picker to offer. */
  it('keeps the Lucidos-only controls off a coding-agent thread', () => {
    const codingAgent = cluster('claude-code');
    expect(codingAgent).toContain(CodingAgentControlMenu);
    expect(codingAgent).not.toContain(LucidosControlMenu);
    expect(codingAgent).not.toContain(TodoListIndicator);
  });

  it('keeps the coding-agent menu off a Lucidos Agent thread', () => {
    const lucidos = cluster(null);
    expect(lucidos).toContain(LucidosControlMenu);
    expect(lucidos).toContain(TodoListIndicator);
    expect(lucidos).not.toContain(CodingAgentControlMenu);
  });

  /** `.prompt-actions-row` is a flex row whose children are diffed
   *  positionally, so the order is part of the contract, not an accident of
   *  how the JSX reads.
   *
   *  The first three slots are FIXED and the rest float behind them: the
   *  control menu anchors the row, the follow toggle is second and the call
   *  toggle third. Each renders in every state. Behind the indicators the
   *  follow toggle was third on a Lucidos Agent thread, second on a
   *  coding-agent one, and fourth with a subscription armed, so the button
   *  moved under the thumb depending on what the thread was doing. The follow
   *  toggle shows up here as its ICON, the one function component inside the
   *  `<button>` this walk can see. The call toggle is its own component, so it
   *  shows up under its own name. */
  it('pins the menu and the two toggles, and floats the indicators behind them', () => {
    expect(cluster(null)).toEqual([
      LucidosControlMenu, FollowLiveEdgeIcon, CallToggle, TodoListIndicator, WaitingIndicator,
    ]);
    expect(cluster('claude-code')).toEqual([
      CodingAgentControlMenu, FollowLiveEdgeIcon, CallToggle, WaitingIndicator,
    ]);
  });
});
