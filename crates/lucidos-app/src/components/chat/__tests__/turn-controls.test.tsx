/** The turn's three controls, which live in the response HEADER.
 *
 *  Two of them were text links at the top of the response body ("More" /
 *  "Less" and "Show steps" / "Hide steps"); the third was a click anywhere on
 *  the header row, announced by nothing but a cursor. What the move bought is
 *  pinned here property by property, because each is easy to lose in a later
 *  edit: the group sits between the executor and the meta cluster, every
 *  control is a real button, and the one control whose effect stops at this
 *  turn says so in its label.
 *
 *  How each states its state is the split that took the most goes. The PAIR
 *  keeps a fixed glyph and brightens (`FullResponseIcon` records why a moving
 *  glyph was wrong for them). The COLLAPSE control is the mirror image: its
 *  glyph flips between a circled minus and plus, and its brightness never
 *  moves. Both halves and the reasons are pinned below, since either one
 *  drifting silently re-breaks a reported bug.
 */
import { describe, expect, it } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { InitiatorPanel, ResponsePanel, turnControls } from '../chat-exchange-parts';
import type { InitiatorDescriptor } from '../ChatExchange';

interface AnyVNode extends VNode<{ children?: ComponentChildren; [k: string]: unknown }> {}

function children(node: ComponentChildren): ComponentChildren[] {
  const kids = (node as AnyVNode)?.props?.children;
  return Array.isArray(kids) ? kids : [kids];
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
  if (String(v.props?.class ?? '').split(/\s+/).includes(cls)) return v;
  return findByClass(v.props?.children, cls);
}

function findByRole(node: ComponentChildren, role: string): AnyVNode | null {
  if (node === null || node === undefined || typeof node === 'boolean') return null;
  if (typeof node === 'string' || typeof node === 'number') return null;
  if (Array.isArray(node)) {
    for (const c of node) {
      const m = findByRole(c, role);
      if (m) return m;
    }
    return null;
  }
  const v = node as AnyVNode;
  if (v.props?.['data-role'] === role) return v;
  return findByRole(v.props?.children, role);
}

const noop = () => {};

const controls = (over: Partial<Parameters<typeof turnControls>[0]> = {}) => turnControls({
  detailsOn: false,
  stepsOn: false,
  collapsed: false,
  collapsible: true,
  onToggleDetails: noop,
  onToggleSteps: noop,
  onToggleCollapsed: noop,
  ...over,
});

const ROLES = ['toggle-details', 'toggle-steps', 'toggle-collapsed'];

/** Every combination of the three states, so a property claimed "in every
 *  state" is checked in all eight rather than in the two anyone thinks of. */
const EVERY_STATE = [false, true].flatMap((detailsOn) =>
  [false, true].flatMap((stepsOn) =>
    [false, true].map((collapsed) => ({ detailsOn, stepsOn, collapsed }))));

describe('turnControls', () => {
  it('renders all three controls in every state', () => {
    // Fixed chrome, not a function of this turn's events. Two of the three flip
    // a per-user setting spanning the transcript, and the third is the only way
    // to unfold a folded turn, so hiding the group while collapsed would strand
    // it. Conditional controls (which is what the text links were) made the
    // group jump between turns and left holes in a column of identical headers.
    for (const state of EVERY_STATE) {
      for (const role of ROLES) {
        expect(findByRole(controls(state), role), `${role} ${JSON.stringify(state)}`).not.toBeNull();
      }
    }
  });

  it('makes every control a real button', () => {
    // Keyboard reachability, and the right semantics for a toggle.
    for (const role of ROLES) {
      const btn = findByRole(controls(), role)!;
      expect(btn.type, role).toBe('button');
      expect(btn.props.type, role).toBe('button');
    }
  });

  it('states each control in aria-pressed', () => {
    // The CSS keys the brightened "on" look off this same attribute, so a
    // control that stops reporting its state also stops looking like it has one.
    const off = controls();
    for (const role of ROLES) expect(findByRole(off, role)!.props['aria-pressed'], role).toBe(false);

    const on = controls({ detailsOn: true, stepsOn: true, collapsed: true });
    for (const role of ROLES) expect(findByRole(on, role)!.props['aria-pressed'], role).toBe(true);
  });

  it('names every control, since none of them carries visible text', () => {
    for (const state of EVERY_STATE) {
      for (const role of ROLES) {
        const btn = findByRole(controls(state), role)!;
        expect(String(btn.props['aria-label'] ?? ''), role).not.toBe('');
        // Desktop hover help. The rule bans native `title` tooltips outright,
        // so this is the only way the icon explains itself before a click.
        expect(btn.props['data-tooltip'], role).toBe(btn.props['aria-label']);
      }
    }
  });

  it('says "this turn" on the one control whose effect stops here', () => {
    // The scope split is the thing a reader has to get right: two of these
    // change every turn in the transcript, one folds the turn it sits on. A gap
    // in the row hints at it; the label is what states it, so the collapse
    // control names the turn and the other two never do.
    const label = (role: string, state = {}) => String(findByRole(controls(state), role)!.props['aria-label']);
    expect(label('toggle-collapsed')).toMatch(/this turn/i);
    expect(label('toggle-collapsed', { collapsed: true })).toMatch(/this turn/i);
    expect(label('toggle-details')).not.toMatch(/this turn/i);
    expect(label('toggle-steps')).not.toMatch(/this turn/i);
  });

  it('disables the collapse control on a turn with no body to fold', () => {
    // `canCollapse` is false while a panel is only a status line. The store
    // would take the fold and hold it, so an enabled control there reads dead
    // on the click and then folds the turn the moment its first content lands.
    // The other two act on a transcript-wide setting, so they never disable.
    const none = controls({ collapsible: false });
    expect(findByRole(none, 'toggle-collapsed')!.props.disabled).toBe(true);
    expect(findByRole(none, 'toggle-details')!.props.disabled).toBeUndefined();
    expect(findByRole(none, 'toggle-steps')!.props.disabled).toBeUndefined();
    expect(findByRole(controls(), 'toggle-collapsed')!.props.disabled).toBe(false);
  });

  it('draws the transcript-wide pair with one fixed glyph, whatever the state', () => {
    // The full-response control started as an unfold/fold pair, on the theory
    // that a fixed glyph cannot say which way the next click goes. The two
    // forms shared a box but not their ink, so the mark visibly changed size
    // on every click while the body under it was also moving, which read as
    // the layout dancing. `aria-pressed` plus the brightness rule carry the
    // state for these two instead.
    for (const role of ['toggle-details', 'toggle-steps']) {
      const glyph = (state: object) => findByRole(controls(state), role)!.props.children as AnyVNode;
      const base = glyph({});
      for (const state of EVERY_STATE) {
        expect(glyph(state).type, role).toBe(base.type);
        expect(glyph(state).props, role).toEqual(base.props);
      }
    }
  });

  /** Invoke the icon with the props the CONTROL handed it, so these cover the
   *  control forwarding its state as well as the icon drawing on it. */
  const drawn = (collapsed: boolean) => {
    const icon = findByRole(controls({ collapsed }), 'toggle-collapsed')!.props.children as AnyVNode;
    const draw = icon.type as (props: Record<string, unknown>) => AnyVNode;
    return (draw(icon.props).props.children as ComponentChildren[])
      .filter((c): c is AnyVNode => typeof c === 'object' && c !== null);
  };
  const strokes = (collapsed: boolean) => drawn(collapsed)
    .filter((n) => n.type === 'line')
    .map((l) => ({ x1: Number(l.props.x1), y1: Number(l.props.y1), x2: Number(l.props.x2), y2: Number(l.props.y2) }));
  const ring = (collapsed: boolean) => drawn(collapsed)
    .filter((n) => n.type === 'circle')
    .map((c) => ({ cx: Number(c.props.cx), cy: Number(c.props.cy), r: Number(c.props.r) }));
  const isHorizontal = (l: ReturnType<typeof strokes>[number]) => l.y1 === l.y2 && l.x1 !== l.x2;
  const isVertical = (l: ReturnType<typeof strokes>[number]) => l.x1 === l.x2 && l.y1 !== l.y2;

  it('draws a circled minus to collapse and a circled plus to expand, matching the tooltip', () => {
    // The one control whose glyph changes, because it is the one with no colour
    // to change: it is exempt from the brightness rule. Assert the MEANING, so
    // a swap of the two forms would contradict its own label and fail here.
    const expanded = strokes(false);
    expect(expanded.length, 'expanded: a lone minus').toBe(1);
    expect(isHorizontal(expanded[0])).toBe(true);

    const collapsed = strokes(true);
    expect(collapsed.length, 'collapsed: a plus').toBe(2);
    expect(collapsed.filter(isHorizontal).length).toBe(1);
    expect(collapsed.filter(isVertical).length).toBe(1);
  });

  it('keeps the ring identical across the flip, with the sign inside it', () => {
    // The circle is what sets the mark's size. If it moved with the state, the
    // flip would read as a resize while the turn under it folds.
    expect(ring(false).length).toBe(1);
    expect(ring(true)).toEqual(ring(false));
    const { cx, cy, r } = ring(false)[0];
    // 20 of 24 units with the stroke is the ceiling this row's glyphs share
    // (see `FollowLiveEdgeIcon`). A bigger mark towers over its neighbours.
    const STROKE = 2;
    expect(2 * r + STROKE).toBeLessThanOrEqual(20);
    for (const collapsed of [false, true]) {
      for (const l of strokes(collapsed)) {
        for (const [x, y] of [[l.x1, l.y1], [l.x2, l.y2]]) {
          expect(Math.hypot(x - cx, y - cy), `collapsed=${collapsed}`).toBeLessThan(r);
        }
      }
    }
  });

  it('reads the three controls as three different actions', () => {
    // Each label changes with its own state, and no two ever coincide: three
    // icons whose hover text agrees are unusable without clicking one.
    const label = (role: string, state = {}) => findByRole(controls(state), role)!.props['aria-label'];
    const on = { detailsOn: true, stepsOn: true, collapsed: true };
    for (const role of ROLES) expect(label(role), role).not.toBe(label(role, on));
    expect(new Set(ROLES.map((r) => label(r))).size).toBe(ROLES.length);
    expect(new Set(ROLES.map((r) => label(r, on))).size).toBe(ROLES.length);
  });
});

describe('ResponsePanel', () => {
  const panel = (slot: ComponentChildren) => ResponsePanel({
    executor: { icon: null, label: 'Claude Code' },
    controls: slot,
    status: null,
    timestamp: '14:32',
    collapsed: false,
    hasBody: true,
    children: null,
  });

  it('puts the controls between the executor and the meta cluster', () => {
    const header = findByClass(panel(controls()), 'response-header')!;
    const positions = children(header).map((c) => {
      const cls = String((c as AnyVNode)?.props?.class ?? '');
      if (cls.includes('response-executor')) return 'executor';
      if (cls.includes('turn-controls')) return 'controls';
      if (cls.includes('response-meta')) return 'meta';
      return null;
    }).filter(Boolean);
    expect(positions).toEqual(['executor', 'controls', 'meta']);
  });

  it('leaves the response header inert, since the collapse control owns folding', () => {
    // The row used to swallow a click to fold the turn, with nothing but a
    // cursor to announce it, under three buttons that each mean something else.
    // A handler here would also fire for any click that misses a button by a
    // pixel, which is most of the row.
    const header = findByClass(panel(controls()), 'response-header')!;
    expect(header.props.onClick).toBeUndefined();
    expect(String(header.props.class)).toBe('response-header');
  });
});

/** The initiator header carries the SAME collapse control. A turn's two headers
 *  are read as one widget. A fold announced by an icon on one and by nothing but
 *  a cursor on the other is two answers to one question. */
describe('InitiatorPanel collapse control', () => {
  const initiator: InitiatorDescriptor = {
    variant: 'system',
    icon: null,
    label: 'Lucidos Agent',
    summary: 'Forwarded message',
  };

  const panel = (over: Partial<Parameters<typeof InitiatorPanel>[0]> = {}) => InitiatorPanel({
    initiator,
    timestamp: '14:32',
    collapsible: true,
    collapsed: false,
    onToggle: noop,
    ...over,
  });

  it('renders the response header\'s collapse control, glyph and label included', () => {
    const here = findByRole(panel(), 'toggle-collapsed')!;
    const there = findByRole(controls(), 'toggle-collapsed')!;
    expect(here).not.toBeNull();
    expect(here.props.class).toBe(there.props.class);
    expect(here.props['aria-label']).toBe(there.props['aria-label']);
    expect(here.props['data-tooltip']).toBe(there.props['data-tooltip']);
    expect((here.props.children as AnyVNode).type).toBe((there.props.children as AnyVNode).type);
  });

  it('turns its glyph around and reports aria-pressed, exactly as the response one', () => {
    const folded = findByRole(panel({ collapsed: true }), 'toggle-collapsed')!;
    expect(folded.props['aria-pressed']).toBe(true);
    expect((folded.props.children as AnyVNode).props.collapsed).toBe(true);
    expect(findByRole(panel(), 'toggle-collapsed')!.props['aria-pressed']).toBe(false);
  });

  it('puts it between the actor chip and the meta cluster, where the response has its run', () => {
    const header = findByClass(panel(), 'initiator-header')!;
    const positions = children(header).map((c) => {
      const cls = String((c as AnyVNode)?.props?.class ?? '');
      if (cls.includes('initiator-actor')) return 'actor';
      if (cls.includes('turn-controls')) return 'controls';
      if (cls.includes('initiator-meta')) return 'meta';
      return null;
    }).filter(Boolean);
    expect(positions).toEqual(['actor', 'controls', 'meta']);
  });

  it('omits it where the response DISABLES its own, because this panel has no body', () => {
    // The response's control renders disabled instead. It is one of three, and
    // a hole in that run is a hole in a column of identical headers. This panel
    // has no run to keep whole. Its body is a pure function of its event and
    // never streams in. A control that never lights up would be a dead icon for
    // the life of the thread.
    expect(findByRole(panel({ collapsible: false, onToggle: undefined }), 'toggle-collapsed')).toBeNull();
  });

  it('keeps that one slot whatever the panel wears, since it has only one', () => {
    // The slot never varied by turn type in the end. The two turns that made
    // it look as though it should, a user message and a change card, carry no
    // fold at all now. `ChatExchange` decides that; this panel renders what it
    // is handed, in the response header's own order.
    const header = findByClass(panel({ bubble: true, chromeless: true }), 'initiator-header')!;
    const slots = children(header).map((c) => {
      const cls = String((c as AnyVNode)?.props?.class ?? '');
      if (cls.includes('turn-controls')) return 'controls';
      if (cls.includes('initiator-meta')) return 'meta';
      return null;
    }).filter(Boolean);
    expect(slots).toEqual(['controls', 'meta']);
  });

  it('leaves the row inert, exactly as the response header does', () => {
    // Both rows once folded on a click announced by nothing but a cursor. A row
    // that swallows one fires wherever the pointer misses a chip, and the
    // control is the affordance now.
    const header = findByClass(panel(), 'initiator-header')!;
    expect(header.props.onClick).toBeUndefined();
    expect(String(header.props.class)).toBe('initiator-header');
  });
});
