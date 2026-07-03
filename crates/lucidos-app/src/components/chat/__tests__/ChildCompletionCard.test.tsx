import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';

vi.mock('../../../store/actions/threads', () => ({
  focusThreadOrBootstrap: vi.fn(),
}));

import { ChildCompletionCard } from '../ChildCompletionCard';
import { focusThreadOrBootstrap } from '../../../store/actions/threads';

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

function findByTag(node: ComponentChildren, tag: string): AnyVNode | null {
  if (node === null || node === undefined || typeof node === 'boolean') return null;
  if (typeof node === 'string' || typeof node === 'number') return null;
  if (Array.isArray(node)) {
    for (const c of node) {
      const m = findByTag(c, tag);
      if (m) return m;
    }
    return null;
  }
  const v = node as AnyVNode;
  if (v.type === tag) return v;
  return findByTag(v.props?.children, tag);
}

const baseProps = {
  childThreadId: 'child-uuid-1',
  childThreadTitle: 'Refactor the foo helper',
  status: 'success' as const,
  summary: 'Cleaned up the if/else ladder.',
};

describe('ChildCompletionCard', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders the verb-prefix + linked title + badge in one header row', () => {
    const tree = ChildCompletionCard(baseProps);
    const row = findByClass(tree, 'child-completion-header-row');
    expect(row).not.toBeNull();
    expect(vnodeText(row)).toContain('Child thread completed:');
    expect(vnodeText(row)).toContain('Refactor the foo helper');
    expect(findByClass(row, 'child-completion-status-success')).not.toBeNull();
  });

  it('title link is an accent-link button with data-thread-id pointing at the child thread', () => {
    const tree = ChildCompletionCard(baseProps);
    const link = findByClass(tree, 'accent-link');
    expect(link).not.toBeNull();
    expect(link!.type).toBe('button');
    expect((link!.props as Record<string, unknown>)['data-thread-id']).toBe('child-uuid-1');
  });

  it('title-link click routes through focusThreadOrBootstrap so an out-of-window child still opens', () => {
    const tree = ChildCompletionCard(baseProps);
    const link = findByClass(tree, 'accent-link');
    expect(link).not.toBeNull();
    const onClick = (link!.props as Record<string, unknown>).onClick as () => void;
    onClick();
    expect(focusThreadOrBootstrap).toHaveBeenCalledWith('child-uuid-1');
  });

  it('falls back to the shared "Untitled thread" placeholder when child_thread_title is missing', () => {
    const tree = ChildCompletionCard({ ...baseProps, childThreadTitle: undefined });
    const link = findByClass(tree, 'accent-link');
    expect(vnodeText(link)).toBe('Untitled thread');
  });

  it('renders agent summary inside a collapsed <details> disclosure', () => {
    const tree = ChildCompletionCard(baseProps);
    const details = findByClass(tree, 'child-completion-disclosure');
    expect(details).not.toBeNull();
    expect(details!.type).toBe('details');
    expect((details!.props as Record<string, unknown>).open).toBeUndefined();
    const summary = findByTag(details, 'summary');
    expect(vnodeText(summary)).toBe('Show summary');
  });

  it('omits the disclosure when the agent summary is empty', () => {
    const tree = ChildCompletionCard({ ...baseProps, summary: '' });
    expect(findByClass(tree, 'child-completion-disclosure')).toBeNull();
  });

  it.each([
    { status: 'success' as const, prefix: 'Child thread completed:', badge: 'child-completion-status-success' },
    { status: 'failure' as const, prefix: 'Child thread failed:', badge: 'child-completion-status-failure' },
    { status: 'no_changes' as const, prefix: 'Child thread completed:', badge: 'child-completion-status-no-changes' },
    { status: 'canceled' as const, prefix: 'Child thread canceled:', badge: 'child-completion-status-canceled' },
  ])('status=$status: prefix "$prefix" with $badge', ({ status, prefix, badge }) => {
    const tree = ChildCompletionCard({ ...baseProps, status });
    const row = findByClass(tree, 'child-completion-header-row');
    expect(vnodeText(row)).toContain(prefix);
    expect(findByClass(row, badge)).not.toBeNull();
  });
});
