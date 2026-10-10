/**
 * The thread filter panel (ThreadFilterPanel), which renders inside the thread
 * drawer pane: the multi-select channel rows, with no heading, then
 * "Include deleted". It shapes the Folders grouping only. These
 * tests invoke the component directly and walk the returned VNode tree WITHOUT
 * descending into the nested function components (<ExpandableChannelRow>,
 * <TriCheckbox>), which use render-time hooks. The component is hook-free at its
 * own level, so direct invocation is safe.
 */
import type { ComponentChildren } from 'preact';
import { beforeEach, describe, expect, it } from 'vitest';
import { ThreadFilterPanel, filterButtonState, FILTER_BUTTON_GLYPHS } from './ThreadFilterPanel';
import {
  ALL_CHANNELS, threadChannelFilter, threadMap, triggers,
  setSelectedTriggerIds, setSelectedRepoIds, setSelectedAppIds,
  setIncludeDeletedFilterOptions,
} from '../../store/store';
import { FilterIcon, FilteredIcon } from '../shared/icons';
import { findByClass, textOf, type AnyVNode } from './__tests__/vnodeWalk';

/** Every DOM `<input>` of a subtree. Same walk as `findByClass`, so it likewise
 *  stops at a function component. */
function findInputs(node: ComponentChildren): AnyVNode[] {
  if (node === null || node === undefined || typeof node !== 'object') return [];
  if (Array.isArray(node)) return node.flatMap(findInputs);
  const v = node as AnyVNode;
  if (typeof v.type !== 'string') return [];
  const out: AnyVNode[] = v.type === 'input' ? [v] : [];
  return out.concat(findInputs(v.props.children as ComponentChildren));
}

/** Every labelled row of a subtree, in render order. */
function rowLabelsInOrder(node: ComponentChildren): string[] {
  if (node === null || node === undefined || typeof node !== 'object') return [];
  if (Array.isArray(node)) return node.flatMap(rowLabelsInOrder);
  const v = node as AnyVNode;
  if (typeof v.type !== 'string') return [];
  const klass = ((v.props.class as string | undefined) ?? '').split(' ');
  if (klass.includes('thread-filter-option')) return [textOf(v)];
  return rowLabelsInOrder(v.props.children as ComponentChildren);
}

function render() {
  // The panel is hook-free at its own level (the Escape registration lives in
  // the store with the open state), so it can be invoked directly.
  const tree = ThreadFilterPanel() as AnyVNode;
  const children = tree.props.children as ComponentChildren;
  const typesGroup = findByClass(children, 'thread-filter-types')[0];
  return { tree, children, typesGroup };
}

beforeEach(() => {
  threadMap.value = new Map();
  // Loaded-but-empty: the option lists return [] until the registry loads.
  triggers.value = { status: 'loaded', data: [] };
  // Every channel selected is the neutral, unfiltered state.
  threadChannelFilter.value = new Set(ALL_CHANNELS);
  setSelectedTriggerIds(new Set());
  setSelectedRepoIds(new Set());
  setSelectedAppIds(new Set());
  setIncludeDeletedFilterOptions(true);
});

describe('ThreadFilterPanel: shape', () => {
  it('is a plain panel element, NOT an <Overlay>', () => {
    // It lives inside the thread drawer pane, so it must not carry the
    // dismiss-and-swallow contract.
    const { tree } = render();
    expect(typeof tree.type).toBe('string');
    expect(tree.props.class).toBe('thread-filter-panel');
  });

  it('carries no heading and no status list: the pane title says Filters', () => {
    const { children } = render();
    expect(findByClass(children, 'thread-filter-title')).toHaveLength(0);
    expect(findByClass(children, 'thread-filter-or')).toHaveLength(0);
    expect(findByClass(children, 'drawer-view-option')).toHaveLength(0);
  });

  it('carries no title row and no footer: the header Filter button is both ends', () => {
    const { children } = render();
    expect(findByClass(children, 'thread-filter-panel-header')).toHaveLength(0);
    expect(findByClass(children, 'thread-filter-panel-footer')).toHaveLength(0);
    expect(findByClass(children, 'thread-filter-close')).toHaveLength(0);
  });

  it('lists the thread types, then Include deleted', () => {
    const rows = rowLabelsInOrder(render().children);
    expect(rows[rows.length - 1]).toBe('Include deleted');
  });
});

describe('ThreadFilterPanel: the thread types', () => {
  it('is a named group, with no heading to name it', () => {
    const group = render().typesGroup;
    expect(group.props.role).toBe('group');
    expect(group.props['aria-label']).toBe('Thread types');
    expect(group.props['aria-labelledby']).toBeUndefined();
  });

  it('is never disabled, and holds only the channel rows', () => {
    const group = render().typesGroup;
    expect(group.props.disabled).toBeUndefined();
    const boxes = findInputs(group);
    expect(boxes.length).toBeGreaterThan(0);
    for (const box of boxes) expect(box.props.disabled).toBeFalsy();
    expect(findByClass(group, 'thread-filter-option').map(textOf)).not.toContain('Include deleted');
  });
});

describe('filterButtonState', () => {
  const shut = (over: Partial<Parameters<typeof filterButtonState>[0]> = {}) => filterButtonState({
    panelOpen: false, channelFilterActive: false, ...over,
  });
  const glyphOf = (over: Partial<Parameters<typeof filterButtonState>[0]> = {}) =>
    FILTER_BUTTON_GLYPHS[shut(over).glyph];

  it('wears the funnel, filled while thread types narrow the list', () => {
    expect(glyphOf()).toBe(FilterIcon);
    expect(glyphOf({ channelFilterActive: true })).toBe(FilteredIcon);
  });

  // The header draws icons in a translucent white. A stroke laid over a fill
  // paints the rim twice, so it reads brighter than the body of the funnel.
  it('paints the filled funnel as one fill, with no stroke over it', () => {
    const svg = FilteredIcon() as { props: { fill: string; stroke: string } };
    expect(svg.props.fill).toBe('currentColor');
    expect(svg.props.stroke).toBe('none');
  });

  // Pressed means the panel is open and nothing else. Never an X: at the far
  // end of the header that reads as "close this pane".
  it('is pressed exactly while the panel is open, wearing the same glyph', () => {
    for (const channelFilterActive of [false, true]) {
      const closed = shut({ channelFilterActive });
      const open = shut({ channelFilterActive, panelOpen: true });
      expect(closed.pressed).toBe(false);
      expect(open.pressed).toBe(true);
      expect(open.glyph).toBe(closed.glyph);
    }
  });

  it('carries no count: the Blocked count rides on the band', () => {
    expect(Object.keys(shut())).toEqual(['glyph', 'pressed']);
  });
});
