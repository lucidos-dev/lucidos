/** A teammate's bubble names its sender and aligns left. The reader's own
 *  bubble, and every bubble on a gateway with nobody to tell apart, draws as
 *  before: a bare timestamp and no teammate class. */
import { describe, expect, it } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { InitiatorPanel } from '../chat-exchange-parts';
import type { InitiatorDescriptor } from '../ChatExchange';

interface AnyVNode extends VNode<{ children?: ComponentChildren; [k: string]: unknown }> {}

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

const initiator: InitiatorDescriptor = { variant: 'user', icon: null, label: 'You', details: 'hello' };

const bubble = (teammate: string | null | undefined) => InitiatorPanel({
  initiator,
  timestamp: 'Today 15:11',
  collapsible: false,
  collapsed: false,
  onActorClick: () => {},
  bubble: true,
  chromeless: true,
  teammate,
});

describe('a teammate\'s bubble', () => {
  it('names the sender before the timestamp and wears the teammate class', () => {
    const panel = bubble('Sam');
    expect(String((panel as AnyVNode).props.class)).toContain('initiator-panel-teammate');
    const sender = findByClass(panel, 'initiator-sender')!;
    expect(sender.props.children).toBe('Sam');
    expect(findByClass(panel, 'initiator-sender-dot')).not.toBeNull();
    expect(findByClass(panel, 'initiator-meta-teammate')).not.toBeNull();
  });

  it('keeps the timestamp a button that opens the origin popover', () => {
    const stamp = findByClass(bubble('Sam'), 'initiator-timestamp-button')!;
    expect(stamp.props.children).toBe('Today 15:11');
    expect(typeof stamp.props.onClick).toBe('function');
  });

  it('draws no sender and no teammate class for the reader\'s own message', () => {
    for (const own of [null, undefined]) {
      const panel = bubble(own);
      expect(String((panel as AnyVNode).props.class)).not.toContain('initiator-panel-teammate');
      expect(findByClass(panel, 'initiator-sender')).toBeNull();
      expect(findByClass(panel, 'initiator-sender-dot')).toBeNull();
    }
  });
});
