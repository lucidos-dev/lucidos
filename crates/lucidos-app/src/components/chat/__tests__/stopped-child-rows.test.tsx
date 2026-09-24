/** The two surfaces of a *stopped child* (ADR 0252): the parent's row for the
 *  `ChildThreadStopped` note, and the notice that ends the child's own
 *  transcript. Both render through the shared event row. */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';

vi.mock('../../../store/actions/threads', () => ({
  focusThreadOrBootstrap: vi.fn(),
}));

import { ChildStoppedRow } from '../ChildCompletionRow';
import { STOPPED_CHILD_CONTINUE, STOPPED_CHILD_SETTLE, StoppedChildNotice } from '../StoppedChildNotice';
import { focusThreadOrBootstrap } from '../../../store/actions/threads';
import type { ThreadMeta } from '../../../store/thread-events';

interface AnyVNode extends VNode<{ children?: ComponentChildren; class?: string; [k: string]: unknown }> {}

function vnodeText(n: ComponentChildren): string {
  if (n === null || n === undefined || typeof n === 'boolean') return '';
  if (typeof n === 'string' || typeof n === 'number') return String(n);
  if (Array.isArray(n)) return n.map(vnodeText).join('');
  return vnodeText((n as AnyVNode).props.children);
}

function findByClass(node: ComponentChildren, cls: string): AnyVNode | null {
  if (node === null || node === undefined || typeof node === 'boolean') return null;
  if (typeof node === 'string' || typeof node === 'number') return null;
  if (Array.isArray(node)) {
    for (const c of node) {
      const m = findByClass(c, cls);
      if (m) return m;
    }
    return null;
  }
  const v = node as AnyVNode;
  const klass = (v.props?.class as string | undefined) ?? '';
  if (typeof klass === 'string' && klass.split(/\s+/).includes(cls)) return v;
  return findByClass(v.props?.children, cls);
}

function meta(overrides: Partial<ThreadMeta>): ThreadMeta {
  return {
    parentThreadId: 'parent-uuid',
    parentThreadTitle: 'Work the ticket',
    isStoppedChild: true,
    ...overrides,
  } as ThreadMeta;
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('ChildStoppedRow', () => {
  /** The parent-side row says the child is alive and waiting, never a verdict:
   *  a canceled pill is what the incident's parent read as "dead". */
  it('says the child stopped and waits for the user, with a link to it', () => {
    const tree = ChildStoppedRow({ childThreadId: 'child-uuid', childThreadTitle: 'Fix the ticket' });
    const row = findByClass(tree, 'event-row');
    expect(row!.props['data-state']).toBe('stopped');
    expect(vnodeText(row)).toContain('Child thread stopped:');
    expect(vnodeText(row)).toContain('Fix the ticket');
    expect(vnodeText(findByClass(tree, 'event-row-state'))).toBe('waiting for you');

    const link = findByClass(tree, 'accent-link')!;
    (link.props as unknown as { onClick: () => void }).onClick();
    expect(focusThreadOrBootstrap).toHaveBeenCalledWith('child-uuid');
  });
});

describe('StoppedChildNotice', () => {
  it('names the waiting parent and both ways forward', () => {
    const tree = StoppedChildNotice({ meta: meta({}) });
    const text = vnodeText(tree);
    expect(text).toContain('Work the ticket');
    expect(text).toContain(STOPPED_CHILD_CONTINUE);
    expect(text).toContain(STOPPED_CHILD_SETTLE);

    const link = findByClass(tree, 'accent-link')!;
    (link.props as unknown as { onClick: () => void }).onClick();
    expect(focusThreadOrBootstrap).toHaveBeenCalledWith('parent-uuid');
  });

  it.each([
    ['a continued child', meta({ isStoppedChild: false })],
    ['a top-thread', meta({ parentThreadId: undefined })],
  ])('renders nothing for %s', (_label, m) => {
    expect(StoppedChildNotice({ meta: m })).toBeNull();
  });
});
