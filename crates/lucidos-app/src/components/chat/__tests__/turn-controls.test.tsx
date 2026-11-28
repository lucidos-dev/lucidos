import { describe, expect, it } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { InitiatorPanel, ResponsePanel, turnControls } from '../chat-exchange-parts';
import type { InitiatorDescriptor } from '../ChatExchange';
import { Disclosure } from '../../shared/Disclosure';
import { ToggleIconButton } from '../../shared/ToggleIconButton';

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

const EVERY_STATE = [false, true].flatMap((detailsOn) =>
  [false, true].flatMap((stepsOn) =>
    [false, true].map((collapsed) => ({ detailsOn, stepsOn, collapsed }))));

describe('turnControls', () => {
  it('renders all three controls in every state', () => {
    for (const state of EVERY_STATE) {
      for (const role of ROLES) {
        expect(findByRole(controls(state), role), `${role} ${JSON.stringify(state)}`).not.toBeNull();
      }
    }
  });

  it('makes every control a toggle icon button', () => {
    for (const role of ROLES) {
      const btn = findByRole(controls(), role)!;
      expect(btn.type, role).toBe(ToggleIconButton);
    }
  });

  it('states each control as pressed', () => {
    const off = controls();
    for (const role of ROLES) expect(findByRole(off, role)!.props.pressed, role).toBe(false);

    const on = controls({ detailsOn: true, stepsOn: true, collapsed: true });
    for (const role of ROLES) expect(findByRole(on, role)!.props.pressed, role).toBe(true);
  });

  it('names every control, since none of them carries visible text', () => {
    for (const state of EVERY_STATE) {
      for (const role of ROLES) {
        const btn = findByRole(controls(state), role)!;
        expect(String(btn.props['aria-label'] ?? ''), role).not.toBe('');
        expect(btn.props['data-tooltip'], role).toBe(btn.props['aria-label']);
      }
    }
  });

  it('says "this turn" on the one control whose effect stops here', () => {
    const label = (role: string, state = {}) => String(findByRole(controls(state), role)!.props['aria-label']);
    expect(label('toggle-collapsed')).toMatch(/this turn/i);
    expect(label('toggle-collapsed', { collapsed: true })).toMatch(/this turn/i);
    expect(label('toggle-details')).not.toMatch(/this turn/i);
    expect(label('toggle-steps')).not.toMatch(/this turn/i);
  });

  it('disables the collapse control on a turn with no body to fold', () => {
    const none = controls({ collapsible: false });
    expect(findByRole(none, 'toggle-collapsed')!.props.disabled).toBe(true);
    expect(findByRole(none, 'toggle-details')!.props.disabled).toBeUndefined();
    expect(findByRole(none, 'toggle-steps')!.props.disabled).toBeUndefined();
    expect(findByRole(controls(), 'toggle-collapsed')!.props.disabled).toBe(false);
  });

  it('draws the transcript-wide pair with one fixed glyph, whatever the state', () => {
    for (const role of ['toggle-details', 'toggle-steps']) {
      const glyph = (state: object) => findByRole(controls(state), role)!.props.children as AnyVNode;
      const base = glyph({});
      for (const state of EVERY_STATE) {
        expect(glyph(state).type, role).toBe(base.type);
        expect(glyph(state).props, role).toEqual(base.props);
      }
    }
  });

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
    const expanded = strokes(false);
    expect(expanded.length, 'expanded: a lone minus').toBe(1);
    expect(isHorizontal(expanded[0])).toBe(true);

    const collapsed = strokes(true);
    expect(collapsed.length, 'collapsed: a plus').toBe(2);
    expect(collapsed.filter(isHorizontal).length).toBe(1);
    expect(collapsed.filter(isVertical).length).toBe(1);
  });

  it('keeps the ring identical across the flip, with the sign inside it', () => {
    expect(ring(false).length).toBe(1);
    expect(ring(true)).toEqual(ring(false));
    const { cx, cy, r } = ring(false)[0];
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
    const header = findByClass(panel(controls()), 'response-header')!;
    expect(header.props.onClick).toBeUndefined();
    expect(String(header.props.class)).toBe('response-header');
  });
});

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
    expect(folded.props.pressed).toBe(true);
    expect((folded.props.children as AnyVNode).props.collapsed).toBe(true);
    expect(findByRole(panel(), 'toggle-collapsed')!.props.pressed).toBe(false);
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
    expect(findByRole(panel({ collapsible: false, onToggle: undefined }), 'toggle-collapsed')).toBeNull();
  });

  it('keeps that one slot whatever the panel wears, since it has only one', () => {
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
    const header = findByClass(panel(), 'initiator-header')!;
    expect(header.props.onClick).toBeUndefined();
    expect(String(header.props.class)).toBe('initiator-header');
  });
});

describe('the fold rolls', () => {
  function disclosures(node: ComponentChildren): AnyVNode[] {
    if (node === null || node === undefined || typeof node === 'boolean') return [];
    if (typeof node === 'string' || typeof node === 'number') return [];
    if (Array.isArray(node)) return node.flatMap(disclosures);
    const v = node as AnyVNode;
    return v.type === Disclosure ? [v] : disclosures(v.props?.children);
  }

  const response = (collapsed: boolean) => ResponsePanel({
    executor: { icon: null, label: 'Claude Code' },
    status: null,
    timestamp: '14:32',
    collapsed,
    hasBody: true,
    children: 'the reply',
  });

  const initiator = (collapsed: boolean) => InitiatorPanel({
    initiator: { variant: 'system', icon: null, label: 'Lucidos Agent', summary: 'Forwarded message' },
    timestamp: '14:32',
    collapsible: true,
    collapsed,
    onToggle: noop,
  });

  it.each([
    ['response', response, 'response-body'],
    ['initiator', initiator, 'initiator-body'],
  ] as const)('rolls the %s body through one disclosure, with nothing in its place', (_name, panel, bodyClass) => {
    for (const collapsed of [false, true]) {
      const rolls = disclosures(panel(collapsed));
      expect(rolls).toHaveLength(1);
      expect(findByClass(rolls[0].props.children, bodyClass)).not.toBeNull();
      expect(rolls[0].props.open).toBe(!collapsed);
    }
  });
});
