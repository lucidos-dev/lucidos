import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';

vi.mock('../../store/actions/threads', () => ({
  focusThreadOrBootstrap: vi.fn(),
}));

import { ThreadLinkButton } from './ChangesView';
import { focusThreadOrBootstrap } from '../../store/actions/threads';

interface AnyVNode extends VNode<{ children?: ComponentChildren; class?: string; [k: string]: unknown }> {}

function vnodeText(n: ComponentChildren): string {
  if (n === null || n === undefined || typeof n === 'boolean') return '';
  if (typeof n === 'string' || typeof n === 'number') return String(n);
  if (Array.isArray(n)) return n.map(vnodeText).join('');
  return vnodeText((n as AnyVNode).props.children);
}

describe('ThreadLinkButton', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders an action-btn labeled "Thread"', () => {
    const node = ThreadLinkButton({ threadId: 'thread-uuid-1' }) as AnyVNode;
    expect(node.type).toBe('button');
    expect(node.props.class).toBe('action-btn');
    expect(vnodeText(node)).toBe('Thread');
  });

  it('click routes through focusThreadOrBootstrap with the change\'s thread id', () => {
    const node = ThreadLinkButton({ threadId: 'thread-uuid-1' }) as AnyVNode;
    const onClick = node.props.onClick as (e: { stopPropagation: () => void }) => void;
    const stopPropagation = vi.fn();
    onClick({ stopPropagation });
    expect(stopPropagation).toHaveBeenCalledTimes(1);
    expect(focusThreadOrBootstrap).toHaveBeenCalledWith('thread-uuid-1');
  });
});
