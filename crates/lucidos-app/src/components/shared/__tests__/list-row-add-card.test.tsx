import { describe, it, expect } from 'vitest';
import type { VNode } from 'preact';
import { ListRowAddCard } from '../ListRowAddCard';
import { vnodeToText } from '../../chat/__tests__/vnodeToText';

/** The rendered root, typed for the props this suite asserts on. There is no
 *  jsdom in the test infra, so the assertions are on the vnode rather than a
 *  mounted element. */
function root(node: unknown) {
  return node as VNode<{ type?: string; class?: string; onClick?: () => void }>;
}

describe('ListRowAddCard', () => {
  it('renders a real button, so the card takes a tab stop and answers Enter and Space', () => {
    const card = root(ListRowAddCard({ label: 'Add Repository', onClick: () => {} }));
    expect(card.type).toBe('button');
    // Without an explicit type a button inside a form submits it.
    expect(card.props.type).toBe('button');
  });

  it('carries the shared class, which is what brings the reset and the focus ring', () => {
    const card = root(ListRowAddCard({ label: 'Add Repository', onClick: () => {} }));
    expect(card.props.class).toBe('list-row-add-card');
  });

  it('shows the + icon and the label, so the label is the accessible name', () => {
    const html = vnodeToText(ListRowAddCard({ label: 'Add Credential', onClick: () => {} }));
    expect(html).toBe(
      '<button class="list-row-add-card">' +
        '<span class="list-row-add-icon">+</span>' +
        '<span class="list-row-add-label">Add Credential</span>' +
        '</button>',
    );
  });

  it('fires onClick', () => {
    let fired = 0;
    const card = root(ListRowAddCard({ label: 'Add Model', onClick: () => { fired++; } }));
    card.props.onClick?.();
    expect(fired).toBe(1);
  });
});
