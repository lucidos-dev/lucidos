import { describe, it, expect } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { changeEventRowBody } from '../chat-exchange-parts';
import { CommitList } from '../../shared/CommitList';
import { EventRowFoldView } from '../EventRow';

interface AnyVNode extends VNode<{ children?: ComponentChildren; class?: string; [k: string]: unknown }> {}

function text(n: ComponentChildren): string {
  if (n === null || n === undefined || typeof n === 'boolean') return '';
  if (typeof n === 'string' || typeof n === 'number') return String(n);
  if (Array.isArray(n)) return n.map(text).join('');
  return text((n as AnyVNode).props.children);
}

function byClass(node: ComponentChildren, cls: string): AnyVNode | null {
  if (node === null || node === undefined || typeof node === 'boolean') return null;
  if (typeof node === 'string' || typeof node === 'number') return null;
  if (Array.isArray(node)) {
    for (const c of node) {
      const m = byClass(c, cls);
      if (m) return m;
    }
    return null;
  }
  const v = node as AnyVNode;
  const klass = v.props?.class;
  if (typeof klass === 'string' && klass.split(/\s+/).includes(cls)) return v;
  return byClass(v.props?.children, cls);
}

function byType(node: ComponentChildren, type: unknown): AnyVNode | null {
  if (node === null || node === undefined || typeof node === 'boolean') return null;
  if (typeof node === 'string' || typeof node === 'number') return null;
  if (Array.isArray(node)) {
    for (const c of node) {
      const m = byType(c, type);
      if (m) return m;
    }
    return null;
  }
  const v = node as AnyVNode;
  if (v.type === type) return v;
  return byType(v.props?.children, type);
}

const base = {
  type: 'ChangeApplied' as const,
  subject: 'Fix the bug',
  stateLabel: 'Applied',
  tone: 'good' as const,
};

/** A change resolution wears the event-row card a child thread's return does. */
describe('change event row', () => {
  it('is an event row: subject, state word, file count', () => {
    const row = changeEventRowBody({ ...base, fileCount: 3 }) as AnyVNode;
    expect(row.props.class).toBe('event-row');
    expect(row.props['data-kind']).toBe('change');
    expect(row.props['data-state']).toBe('ChangeApplied');
    expect(text(byClass(row, 'event-row-subject'))).toBe('Fix the bug');
    const state = byClass(row, 'event-row-state')!;
    expect(text(state)).toBe('Applied');
    expect(state.props['data-tone']).toBe('good');
    expect(text(byClass(row, 'event-row-meta'))).toBe('3 files');
  });

  it('puts Diff and Revert inside the card', () => {
    const row = changeEventRowBody({ ...base, actions: <button class="action-btn">Revert</button> });
    expect(text(byClass(row, 'event-row-actions'))).toBe('Revert');
  });

  /** Only a verdict earns a glyph. A revert is a state, not a pass or a fail. */
  it.each([
    ['good', true],
    ['bad', true],
    ['none', false],
    ['halted', false],
  ] as const)('draws a glyph before the %s state word: %s', (tone, glyph) => {
    const row = changeEventRowBody({ ...base, tone });
    const state = byClass(row, 'event-row-state')!;
    expect(byClass(state, 'event-row-state-glyph') !== null).toBe(glyph);
    expect(text(state)).toBe('Applied');
  });

  it('shows a failed apply\'s error unfolded', () => {
    const row = changeEventRowBody({
      ...base,
      type: 'ChangeApplyFailed',
      subject: 'Change',
      stateLabel: 'Failed',
      tone: 'bad',
      error: 'merge conflict in a.ts',
    });
    const fold = byType(row, EventRowFoldView)!;
    expect(fold.props.open).toBe(true);
    expect(text(fold.props.body as ComponentChildren)).toContain('merge conflict in a.ts');
  });

  /** The headline names the change; the commits it is made of sit one fold
   *  away, closed, oldest first. */
  it('folds a change of several commits into a closed commit list', () => {
    const row = changeEventRowBody({ ...base, commits: ['feat: first', 'fix: second'] });
    const fold = byType(row, EventRowFoldView)!;
    expect(fold.props.label).toBe('2 commits');
    expect(fold.props.open).toBeFalsy();
    const list = byType(fold.props.body as ComponentChildren, CommitList)!;
    expect(list.props.commits).toEqual(['feat: first', 'fix: second']);
  });

  it('draws no fold for one commit, which the headline already is', () => {
    const row = changeEventRowBody({ ...base, commits: ['feat: only'] });
    expect(byType(row, EventRowFoldView)).toBeNull();
  });

  it('gives the fold to a failed apply\'s error over the commit list', () => {
    const row = changeEventRowBody({
      ...base,
      type: 'ChangeApplyFailed',
      stateLabel: 'Failed',
      tone: 'bad',
      commits: ['a', 'b'],
      error: 'merge conflict',
    });
    expect(byType(row, EventRowFoldView)!.props.label).toBe('Error');
  });

  it('states no fact it does not have', () => {
    const row = changeEventRowBody(base);
    expect(byClass(row, 'event-row-meta')).toBeNull();
    expect(byType(row, EventRowFoldView)).toBeNull();
    expect(byClass(row, 'event-row-actions')).toBeNull();
  });
});
