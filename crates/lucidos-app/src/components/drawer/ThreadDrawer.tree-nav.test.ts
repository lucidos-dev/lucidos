import { describe, it, expect, afterEach } from 'vitest';
import {
  leftAction, rightAction, nodeKey, sectionNavKey, handleDrawerKeyDown,
  type DrawerNavNode, type NavCollapseState,
} from './ThreadDrawer';
import { openThreadFilterPanel, closeThreadFilterPanel } from '../../store/threadFilterPanel';

// Pure ←/→ tree-navigation logic for the drawer — no signals, no DOM.

const state = (sections: string[] = [], families: string[] = []): NavCollapseState => ({
  sectionCollapsed: (k) => sections.includes(k),
  familyCollapsed: (id) => families.includes(id),
});

const section = (sectionKey: 'saved' | 'current' | 'archive'): DrawerNavNode =>
  ({ kind: 'section', sectionKey });

const thread = (
  id: string,
  depth: number,
  parentId: string | null,
  hasChildren: boolean,
  sectionKey: 'saved' | 'current' | 'archive' | null,
): DrawerNavNode => ({ kind: 'thread', id, depth, parentId, hasChildren, sectionKey });

describe('nodeKey / sectionNavKey', () => {
  it('keys a section header by its prefixed name', () => {
    expect(sectionNavKey('current')).toBe('__section_current');
    expect(nodeKey(section('current'))).toBe('__section_current');
  });
  it('keys a thread by its id', () => {
    expect(nodeKey(thread('t-1', 0, null, false, 'current'))).toBe('t-1');
  });
});

describe('leftAction (collapse / ascend)', () => {
  it('collapses an expanded section, focusing its header', () => {
    expect(leftAction(section('current'), state())).toEqual({ type: 'collapseSection', sectionKey: 'current' });
  });
  it('is a no-op on an already-collapsed section', () => {
    expect(leftAction(section('current'), state(['current']))).toEqual({ type: 'none' });
  });
  it('collapses a top-level parent’s own family first, staying put', () => {
    expect(leftAction(thread('p', 0, null, true, 'current'), state())).toEqual({
      type: 'collapseFamily', threadId: 'p', focusKey: 'p',
    });
  });
  it('collapses the section once a top-level parent’s family is already collapsed', () => {
    expect(leftAction(thread('p', 0, null, true, 'current'), state([], ['p']))).toEqual({
      type: 'collapseSection', sectionKey: 'current',
    });
  });
  it('collapses the section from a top-level leaf', () => {
    expect(leftAction(thread('t', 0, null, false, 'current'), state())).toEqual({
      type: 'collapseSection', sectionKey: 'current',
    });
  });
  it('on a sub-thread, collapses the PARENT family (child + siblings) and focuses the parent', () => {
    expect(leftAction(thread('c', 1, 'p', false, 'current'), state())).toEqual({
      type: 'collapseFamily', threadId: 'p', focusKey: 'p',
    });
  });
  it('on a sub-thread that is itself an expanded parent, collapses its own family first', () => {
    expect(leftAction(thread('c', 1, 'p', true, 'current'), state())).toEqual({
      type: 'collapseFamily', threadId: 'c', focusKey: 'c',
    });
  });
  it('is inert on a flat-view node (no section)', () => {
    expect(leftAction(thread('t', 0, null, false, null), state())).toEqual({ type: 'none' });
  });

  it('cascades child → parent family → section (the spec example)', () => {
    // ← on a child collapses the parent's family and focuses the parent.
    expect(leftAction(thread('c-1', 1, 'p-1', false, 'current'), state())).toEqual({
      type: 'collapseFamily', threadId: 'p-1', focusKey: 'p-1',
    });
    // ← again, now on the parent (family collapsed), collapses the section.
    expect(leftAction(thread('p-1', 0, null, true, 'current'), state([], ['p-1']))).toEqual({
      type: 'collapseSection', sectionKey: 'current',
    });
  });
});

describe('rightAction (expand / descend)', () => {
  it('expands a collapsed section, staying on the header', () => {
    const nodes = [section('current')];
    expect(rightAction(nodes[0], nodes, 0, state(['current']))).toEqual({ type: 'expandSection', sectionKey: 'current' });
  });
  it('descends an expanded section to its first thread', () => {
    const nodes = [section('current'), thread('t', 0, null, false, 'current')];
    expect(rightAction(nodes[0], nodes, 0, state())).toEqual({ type: 'focusKey', key: 't' });
  });
  it('is a no-op on an expanded section with no following thread', () => {
    const nodes = [section('current')];
    expect(rightAction(nodes[0], nodes, 0, state())).toEqual({ type: 'none' });
  });
  it('expands a collapsed family, staying on the parent', () => {
    const nodes = [thread('p', 0, null, true, 'current')];
    expect(rightAction(nodes[0], nodes, 0, state([], ['p']))).toEqual({ type: 'expandFamily', threadId: 'p' });
  });
  it('descends an expanded parent to its first child', () => {
    const nodes = [thread('p', 0, null, true, 'current'), thread('c', 1, 'p', false, 'current')];
    expect(rightAction(nodes[0], nodes, 0, state())).toEqual({ type: 'focusKey', key: 'c' });
  });
  it('is a no-op on an expanded parent whose next row is a sibling (no rendered child)', () => {
    const nodes = [thread('p', 0, null, true, 'current'), thread('s', 0, null, false, 'current')];
    expect(rightAction(nodes[0], nodes, 0, state())).toEqual({ type: 'none' });
  });
  it('is a no-op on a leaf thread', () => {
    const nodes = [thread('t', 0, null, false, 'current')];
    expect(rightAction(nodes[0], nodes, 0, state())).toEqual({ type: 'none' });
  });
});

describe('handleDrawerKeyDown: filter-panel suppression', () => {
  // The filter panel is a view inside this pane, covering the list, and its rows
  // are real controls. Their keys bubble out to the pane container, so the
  // container's list-nav has to stand down while the panel is up: otherwise
  // Enter on a View row would ALSO open whatever thread the invisible list
  // happens to have highlighted.
  const keyEvent = (key: string) => {
    let prevented = false;
    const e = { key, preventDefault: () => { prevented = true; } } as unknown as KeyboardEvent;
    return { e, wasPrevented: () => prevented };
  };

  afterEach(() => closeThreadFilterPanel());

  it('consumes Enter and the vertical arrows while the panel is closed', () => {
    for (const key of ['Enter', 'ArrowDown', 'ArrowUp']) {
      const { e, wasPrevented } = keyEvent(key);
      handleDrawerKeyDown(e);
      expect(wasPrevented(), key).toBe(true);
    }
  });

  it('acts on nothing while the panel is open', () => {
    openThreadFilterPanel();
    for (const key of ['Enter', 'ArrowDown', 'ArrowUp', 'ArrowLeft', 'ArrowRight']) {
      const { e, wasPrevented } = keyEvent(key);
      handleDrawerKeyDown(e);
      expect(wasPrevented(), key).toBe(false);
    }
  });
});
