import { describe, expect, it } from 'vitest';
import type { ComponentChild, VNode } from 'preact';
import {
  CollapsingActions,
  renderHeaderAction,
  renderMenuAction,
  type HeaderActionSpec,
} from '../headerActions';
import { OverflowMenu, type OverflowMenuContext } from '../../shared/OverflowMenu';
import { ToggleIconButton } from '../../shared/ToggleIconButton';

type Props = Record<string, unknown>;
const propsOf = (node: ComponentChild): Props => (node as VNode<Props>).props;

const TRIGGER = { id: 'the-more-trigger' } as unknown as HTMLElement;
const ctx: OverflowMenuContext = {
  run: (fn) => () => fn(),
  anchor: TRIGGER,
};

const spec = (over: Partial<HeaderActionSpec> = {}): HeaderActionSpec => ({
  key: 'probe',
  label: 'Probe',
  icon: () => null,
  onClick: () => {},
  ...over,
});

describe('data-role travels with the action', () => {
  it('reaches the row button', () => {
    expect(propsOf(renderHeaderAction(spec({ dataRole: 'probe-role' })))['data-role'])
      .toBe('probe-role');
  });

  it('reaches the ⋯ menu row', () => {
    expect(propsOf(renderMenuAction(spec({ dataRole: 'probe-role' }), ctx))['data-role'])
      .toBe('probe-role');
  });

  it('reaches a disabled action on both sides', () => {
    const disabled = spec({ dataRole: 'probe-role', disabledTooltip: 'nope' });
    expect(propsOf(renderHeaderAction(disabled))['data-role']).toBe('probe-role');
    expect(propsOf(renderMenuAction(disabled, ctx))['data-role']).toBe('probe-role');
  });
});

describe('a toggle says so, on both sides', () => {
  it('renders a toggle through ToggleIconButton, framed by default', () => {
    const node = renderHeaderAction(spec({ active: true })) as VNode<Props>;
    expect(node.type).toBe(ToggleIconButton);
    expect(node.props.tone).toBeUndefined();
    expect(node.props.class).toBe('header-icon');
  });

  it('hands the tone of the spec on', () => {
    expect(propsOf(renderHeaderAction(spec({ active: true, tone: 'glyph' }))).tone).toBe('glyph');
  });

  it('announces the row button as pressed, in either state', () => {
    expect(propsOf(renderHeaderAction(spec({ active: true }))).pressed).toBe(true);
    expect(propsOf(renderHeaderAction(spec({ active: false }))).pressed).toBe(false);
  });

  it('leaves a plain action unpressed rather than off', () => {
    expect(propsOf(renderHeaderAction(spec()))['aria-pressed']).toBeUndefined();
  });

  it('gives the menu row a checkable role', () => {
    const on = propsOf(renderMenuAction(spec({ active: true }), ctx));
    expect(on.role).toBe('menuitemcheckbox');
    expect(on['aria-checked']).toBe(true);
  });

  it('leaves a plain action a plain menu item', () => {
    const plain = propsOf(renderMenuAction(spec(), ctx));
    expect(plain.role).toBe('menuitem');
    expect(plain['aria-checked']).toBeUndefined();
  });
});

describe('a menu row keeps its words in a clamped box', () => {
  const labelOf = (node: ComponentChild): ComponentChild =>
    (propsOf(node).children as ComponentChild[])[1];

  it('boxes the label on every rendering', () => {
    for (const over of [{}, { href: '/x' }, { disabledTooltip: 'nope' }]) {
      const label = labelOf(renderMenuAction(spec({ ...over, label: 'Probe' }), ctx));
      expect(propsOf(label).class).toBe('thread-overflow-label');
      expect(propsOf(label).children).toBe('Probe');
    }
  });
});

describe('a press can leave focus where it is', () => {
  const mouseDownOf = (node: ComponentChild) =>
    propsOf(node).onMouseDown as ((e: Event) => void) | undefined;

  it('cancels the mousedown default for a spec that keeps focus', () => {
    let prevented = false;
    mouseDownOf(renderHeaderAction(spec({ keepsFocus: true })))!(
      { preventDefault: () => { prevented = true; } } as Event,
    );
    expect(prevented).toBe(true);
  });

  it('leaves a plain action to take focus as usual', () => {
    expect(mouseDownOf(renderHeaderAction(spec()))).toBeUndefined();
  });
});

describe('a menu row that OPENS something is handed the trigger', () => {
  it('runs onMenuClick against the ⋯, not the row', () => {
    let seen: HTMLElement | null | undefined;
    const row = renderMenuAction(spec({ onMenuClick: (a) => { seen = a; } }), ctx);
    (propsOf(row).onClick as (e: MouseEvent) => void)({} as MouseEvent);
    expect(seen).toBe(TRIGGER);
  });

  it('falls back to onClick for an action that opens nothing', () => {
    let fired = false;
    const row = renderMenuAction(spec({ onClick: () => { fired = true; } }), ctx);
    (propsOf(row).onClick as (e: MouseEvent) => void)({} as MouseEvent);
    expect(fired).toBe(true);
  });
});

describe('a disabled menu row stays focusable, so arrow keys can reach it', () => {
  it('marks it aria-disabled, never with the disabled attribute, and gives it no press', () => {
    const row = propsOf(renderMenuAction(spec({ disabledTooltip: 'nope' }), ctx));
    expect(row['aria-disabled']).toBe('true');
    expect(row.disabled).toBeUndefined();
    expect(row.onClick).toBeUndefined();
  });
});

describe('a custom row rendering is handed the same attributes', () => {
  it('receives the cluster attrs and the data-role together', () => {
    let seen: Record<string, string> | null = null;
    renderHeaderAction(
      spec({ dataRole: 'probe-role', render: (attrs) => { seen = attrs; return null; } }),
      { 'data-row-item': 'fold' },
    );
    expect(seen).toEqual({ 'data-row-item': 'fold', 'data-role': 'probe-role' });
  });
});

describe('CollapsingActions stamps its cluster attrs on every in-row member', () => {
  const actions = [spec({ key: 'a' }), spec({ key: 'b' }), spec({ key: 'c' })];
  const attrs = { 'data-row-item': 'fold' };
  const childrenOf = (collapsed: number): ComponentChild[] => {
    const tree = CollapsingActions({ actions, collapsed, moreClass: 'probe-more', itemAttrs: attrs });
    return (propsOf(tree).children as ComponentChild[]).flat();
  };

  it('reaches the actions still wearing their own icon', () => {
    const visible = childrenOf(2).filter((c) => (c as VNode)?.key === 'c');
    expect(visible).toHaveLength(1);
    expect(propsOf(propsOf(visible[0]).children as ComponentChild)['data-row-item']).toBe('fold');
  });

  it('reaches the ⋯ trigger', () => {
    const more = childrenOf(2).find((c) => (c as VNode)?.type === OverflowMenu);
    expect(more, 'no ⋯ menu rendered for a collapsed cluster').toBeTruthy();
    expect(propsOf(more!).triggerAttrs).toBe(attrs);
  });

  it('draws no ⋯ while nothing is folded', () => {
    expect(childrenOf(0).find((c) => (c as VNode)?.type === OverflowMenu)).toBeUndefined();
  });
});
