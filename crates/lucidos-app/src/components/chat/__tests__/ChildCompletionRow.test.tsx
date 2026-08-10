import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';

vi.mock('../../../store/actions/threads', () => ({
  focusThreadOrBootstrap: vi.fn(),
}));

import { ChildCompletionRow } from '../ChildCompletionRow';
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

describe('ChildCompletionRow', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  /** The callback shares ONE marker with the event wait, the event wake and the
   *  trigger fire: all four report that something happened outside this thread,
   *  and they used to say it in four dialects. This pins that it is the shared
   *  row rather than a card of its own again. */
  it('is an event row: verb-prefix, linked title, state word', () => {
    const tree = ChildCompletionRow(baseProps);
    const row = findByClass(tree, 'event-row');
    expect(row).not.toBeNull();
    expect(row!.props['data-kind']).toBe('child');
    expect(row!.props['data-state']).toBe('success');
    expect(vnodeText(row)).toContain('Child thread returned:');
    expect(vnodeText(row)).toContain('Refactor the foo helper');
    // The one event row that legitimately shows a verdict, because the verdict
    // it reports is the CHILD's outcome rather than the row's own.
    const pill = findByClass(tree, 'event-row-state');
    expect(vnodeText(pill)).toBe('success');
    expect(pill!.props['data-tone']).toBe('good');
  });

  /** Straight off `pending_change_ids`. A row written before that field existed
   *  omits the count rather than claiming zero: a row states no fact its event
   *  does not carry. */
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

  /** One fold, labelled by what it holds, the same as the wake's `Payload` and
   *  the trigger's `Prompt`. It was "Show summary", which is an instruction
   *  where the other two are nouns. */
  it('renders agent summary inside a collapsed <details> disclosure', () => {
    const tree = ChildCompletionRow(baseProps);
    const details = findByClass(tree, 'event-row-fold');
    expect(details).not.toBeNull();
    expect(details!.type).toBe('details');
    expect((details!.props as Record<string, unknown>).open).toBeUndefined();
    const summary = findByTag(details, 'summary');
    expect(vnodeText(summary)).toBe('Summary');
  });

  it('omits the disclosure when the agent summary is empty', () => {
    const tree = ChildCompletionRow({ ...baseProps, summary: '' });
    expect(findByClass(tree, 'event-row-fold')).toBeNull();
  });

  /** The four appear together in one stream, so each has to be distinguishable
   *  from the other three. The verb carries the shape of what happened and the
   *  state word carries the verdict, so `success` and `no_changes` share a verb
   *  and are still told apart. */
  it.each([
    { status: 'success' as const, prefix: 'Child thread returned:', word: 'success', tone: 'good' },
    { status: 'failure' as const, prefix: 'Child thread failed:', word: 'failure', tone: 'bad' },
    { status: 'no_changes' as const, prefix: 'Child thread returned:', word: 'no changes', tone: 'none' },
    { status: 'canceled' as const, prefix: 'Child thread canceled:', word: 'canceled', tone: 'halted' },
  ])('status=$status: prefix "$prefix" with the word "$word"', ({ status, prefix, word, tone }) => {
    const tree = ChildCompletionRow({ ...baseProps, status });
    expect(vnodeText(findByClass(tree, 'event-row-subject'))).toContain(prefix);
    const pill = findByClass(tree, 'event-row-state');
    expect(vnodeText(pill)).toBe(word);
    expect(pill!.props['data-tone']).toBe(tone);
  });
});
