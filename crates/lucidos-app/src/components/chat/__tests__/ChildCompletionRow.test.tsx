// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';

vi.mock('../../../store/actions/threads', () => ({
  focusThreadOrBootstrap: vi.fn(),
}));

import { ChildCompletionRow, FILED_REPORT_LINE } from '../ChildCompletionRow';
import { EventRowFoldView } from '../EventRow';
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

function findByType(node: ComponentChildren, type: unknown): AnyVNode | null {
  if (node === null || node === undefined || typeof node === 'boolean') return null;
  if (typeof node === 'string' || typeof node === 'number') return null;
  if (Array.isArray(node)) {
    for (const c of node) {
      const m = findByType(c, type);
      if (m) return m;
    }
    return null;
  }
  const v = node as AnyVNode;
  if (v.type === type) return v;
  return findByType(v.props?.children, type);
}

const baseProps = {
  childThreadId: 'child-uuid-1',
  childThreadTitle: 'Refactor the foo helper',
  status: 'success' as const,
  summary: 'Cleaned up the if/else ladder.',
};

describe('ChildCompletionRow', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('is an event row: pill, linked title, state word', () => {
    const tree = ChildCompletionRow(baseProps);
    const row = findByClass(tree, 'event-row');
    expect(row).not.toBeNull();
    expect(row!.props['data-kind']).toBe('child');
    expect(row!.props['data-state']).toBe('success');
    expect(vnodeText(findByClass(tree, 'event-row-subject'))).toBe('Child thread returned');
    expect(vnodeText(findByClass(tree, 'event-row-meta'))).toContain('Refactor the foo helper');
    const pill = findByClass(tree, 'event-row-state');
    expect(vnodeText(pill)).toBe('Success');
    expect(pill!.props['data-tone']).toBe('good');
  });

  it.each<[string[] | undefined, string | undefined]>([
    [undefined, undefined],
    [[], undefined],
    [['c1'], '1 pending change'],
    [['c1', 'c2'], '2 pending changes'],
  ])('reports %s as %s', (ids, expected) => {
    const text = vnodeText(ChildCompletionRow({ ...baseProps, pendingChangeIds: ids }));
    if (expected) expect(text).toContain(expected);
    else expect(text).not.toContain('pending change');
  });

  it('counts the changes pending in its sub-threads apart from its own', () => {
    const below = (n: number) =>
      Array.from({ length: n }, (_, i) => ({
        change_id: `c${i}`,
        thread_id: `t${i}`,
        thread_unsettled: false,
      }));
    const text = vnodeText(ChildCompletionRow({ ...baseProps, subThreadPendingChanges: below(2) }));
    expect(text).toContain('2 pending in its sub-threads');
    expect(text).not.toContain('pending change');
    expect(vnodeText(ChildCompletionRow(baseProps))).not.toContain('sub-threads');
  });

  it('says a filed report did not wake this thread', () => {
    const filed = ChildCompletionRow({ ...baseProps, filed: true });
    expect(vnodeText(findByClass(filed, 'event-row-subject'))).toBe('Child thread report filed');
    expect(vnodeText(filed)).toContain(FILED_REPORT_LINE);
    expect(vnodeText(ChildCompletionRow(baseProps))).not.toContain(FILED_REPORT_LINE);
  });

  it('title link is an accent-link button with data-thread-id pointing at the child thread', () => {
    const tree = ChildCompletionRow(baseProps);
    const link = findByClass(tree, 'accent-link');
    expect(link).not.toBeNull();
    expect(link!.type).toBe('button');
    expect((link!.props as Record<string, unknown>)['data-thread-id']).toBe('child-uuid-1');
  });

  it('title-link click routes through focusThreadOrBootstrap so an out-of-window child still opens', () => {
    const tree = ChildCompletionRow(baseProps);
    const link = findByClass(tree, 'accent-link');
    expect(link).not.toBeNull();
    const onClick = (link!.props as Record<string, unknown>).onClick as () => void;
    onClick();
    expect(focusThreadOrBootstrap).toHaveBeenCalledWith('child-uuid-1');
  });

  it('falls back to the shared "Untitled thread" placeholder when child_thread_title is missing', () => {
    const tree = ChildCompletionRow({ ...baseProps, childThreadTitle: undefined });
    const link = findByClass(tree, 'accent-link');
    expect(vnodeText(link)).toBe('Untitled thread');
  });

  it('renders agent summary inside a collapsed fold', () => {
    const tree = ChildCompletionRow(baseProps);
    const fold = findByType(tree, EventRowFoldView);
    expect(fold).not.toBeNull();
    expect(fold!.props.open).toBeUndefined();
    expect(fold!.props.label).toBe('Details');
  });

  it('omits the disclosure when the agent summary is empty', () => {
    const tree = ChildCompletionRow({ ...baseProps, summary: '' });
    expect(findByType(tree, EventRowFoldView)).toBeNull();
  });

  it.each([
    { status: 'success' as const, word: 'Success', tone: 'good' },
    { status: 'failure' as const, word: 'Failure', tone: 'bad' },
    { status: 'no_changes' as const, word: 'No changes', tone: 'none' },
    { status: 'canceled' as const, word: 'Canceled', tone: 'halted' },
    { status: 'interrupted' as const, word: 'Interrupted', tone: 'halted' },
  ])('status=$status reads "$word"', ({ status, word, tone }) => {
    const tree = ChildCompletionRow({ ...baseProps, status });
    expect(vnodeText(findByClass(tree, 'event-row-subject'))).toBe('Child thread returned');
    const pill = findByClass(tree, 'event-row-state');
    expect(vnodeText(pill)).toBe(word);
    expect(pill!.props['data-tone']).toBe(tone);
  });
});
