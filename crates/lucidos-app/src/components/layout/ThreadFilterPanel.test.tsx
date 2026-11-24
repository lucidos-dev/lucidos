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

function findInputs(node: ComponentChildren): AnyVNode[] {
  if (node === null || node === undefined || typeof node !== 'object') return [];
  if (Array.isArray(node)) return node.flatMap(findInputs);
  const v = node as AnyVNode;
  if (typeof v.type !== 'string') return [];
  const out: AnyVNode[] = v.type === 'input' ? [v] : [];
  return out.concat(findInputs(v.props.children as ComponentChildren));
}

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
  const tree = ThreadFilterPanel() as AnyVNode;
  const children = tree.props.children as ComponentChildren;
  const typesGroup = findByClass(children, 'thread-filter-types')[0];
  return { tree, children, typesGroup };
}

beforeEach(() => {
  threadMap.value = new Map();
  triggers.value = { status: 'loaded', data: [] };
  threadChannelFilter.value = new Set(ALL_CHANNELS);
  setSelectedTriggerIds(new Set());
  setSelectedRepoIds(new Set());
  setSelectedAppIds(new Set());
  setIncludeDeletedFilterOptions(true);
});

describe('ThreadFilterPanel: shape', () => {
  it('is a plain panel element, NOT an <Overlay>', () => {
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

  it('paints the filled funnel as one fill, with no stroke over it', () => {
    const svg = FilteredIcon() as { props: { fill: string; stroke: string } };
    expect(svg.props.fill).toBe('currentColor');
    expect(svg.props.stroke).toBe('none');
  });

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
