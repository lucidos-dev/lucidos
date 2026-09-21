/** What a `HeaderActionSpec` promises to carry into BOTH of its renderings.
 *
 *  An action folds into the ⋯ menu the moment the row runs short of room. So
 *  anything that addresses it has to survive the fold. That was already true of
 *  `extraClass`. It is now true of `data-role` too, which is how the composer's
 *  toggles are addressed by their CSS and by the e2e suite.
 *
 *  A vnode walk, not a mount: every property here is an attribute the renderer
 *  puts on the element, so the props are the whole of the answer.
 *
 *  Plan: `docs/plans/2026-09-19-composer-icon-row-overflow-menu.md`. */
import { describe, expect, it } from 'vitest';
import type { ComponentChild, VNode } from 'preact';
import {
  CollapsingActions,
  renderHeaderAction,
  renderMenuAction,
  type HeaderActionSpec,
} from '../headerActions';
import { OverflowMenu, type OverflowMenuContext } from '../../shared/OverflowMenu';

type Props = Record<string, unknown>;
const propsOf = (node: ComponentChild): Props => (node as VNode<Props>).props;

const TRIGGER = { id: 'the-more-trigger' } as unknown as HTMLElement;
const ctx: OverflowMenuContext = {
  open: true,
  openedViaKeyboard: false,
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
  it('paints the row button with filter-active by default', () => {
    expect(propsOf(renderHeaderAction(spec({ active: true }))).class)
      .toContain('filter-active');
  });

  /** The composer paints each toggle from its own `data-role`, and the header's
   *  frame would fight those rules. */
  it('takes the class the spec asks for instead', () => {
    expect(propsOf(renderHeaderAction(spec({ active: true, activeClass: 'active' }))).class)
      .toBe('icon-btn header-icon active');
  });

  it('announces the row button as pressed, in either state', () => {
    expect(propsOf(renderHeaderAction(spec({ active: true })))['aria-pressed']).toBe(true);
    expect(propsOf(renderHeaderAction(spec({ active: false })))['aria-pressed']).toBe(false);
  });

  it('leaves a plain action unpressed rather than off', () => {
    expect(propsOf(renderHeaderAction(spec()))['aria-pressed']).toBeUndefined();
  });

  /** Colour is the only channel the row button has, and a menu row has none at
   *  all. `menuitemcheckbox` is the one menu role able to hold `aria-checked`,
   *  and `OverflowMenu` roves it like any other item. */
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

/** A folded STATUS readout reports in words, and the waiting indicator's words
 *  are the model's own sentence. Left as a bare text node it had no box to clamp,
 *  so it wrapped to five lines of a three-row menu. */
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

describe('a menu row that OPENS something is handed the trigger', () => {
  /** The row it was clicked on unmounts as the menu closes, so a popover
   *  anchored to it would be positioned against nothing. The ⋯ is the only box
   *  still standing. */
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

  /** The trigger stands in for whatever folded, so the measurement has to see
   *  it as a member too. Otherwise the row reads one box narrower than it is. */
  it('reaches the ⋯ trigger', () => {
    const more = childrenOf(2).find((c) => (c as VNode)?.type === OverflowMenu);
    expect(more, 'no ⋯ menu rendered for a collapsed cluster').toBeTruthy();
    expect(propsOf(more!).triggerAttrs).toBe(attrs);
  });

  it('draws no ⋯ while nothing is folded', () => {
    expect(childrenOf(0).find((c) => (c as VNode)?.type === OverflowMenu)).toBeUndefined();
  });
});
