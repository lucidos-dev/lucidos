import { describe, it, expect } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { linkifyText } from '../linkifyText';

type AnchorProps = {
  href?: string;
  target?: string;
  rel?: string;
  children?: ComponentChildren;
};

function isAnchor(node: ComponentChildren): node is VNode<AnchorProps> {
  return (
    typeof node === 'object' &&
    node !== null &&
    (node as VNode).type === 'a'
  );
}

/** Normalize the result to an array so string and array returns test uniformly. */
function toArray(result: ComponentChildren): ComponentChildren[] {
  return Array.isArray(result) ? result : [result];
}

describe('linkifyText', () => {
  it('returns the original string unchanged when there is no URL', () => {
    const result = linkifyText('just some plain text, no links here');
    expect(result).toBe('just some plain text, no links here');
  });

  it('turns a bare URL into an anchor that opens in a new tab', () => {
    const parts = toArray(linkifyText('Learn more: https://example.com/news/x'));
    expect(parts[0]).toBe('Learn more: ');
    const anchor = parts[1];
    expect(isAnchor(anchor)).toBe(true);
    if (!isAnchor(anchor)) return;
    expect(anchor.props.href).toBe('https://example.com/news/x');
    expect(anchor.props.target).toBe('_blank');
    expect(anchor.props.rel).toBe('noopener noreferrer');
    expect(anchor.props.children).toBe('https://example.com/news/x');
  });

  it('keeps trailing sentence punctuation outside the link', () => {
    const parts = toArray(linkifyText('See https://example.com/page.'));
    const anchor = parts.find(isAnchor);
    expect(anchor?.props.href).toBe('https://example.com/page');
    // The period is preserved as trailing plain text.
    expect(parts[parts.length - 1]).toBe('.');
  });

  it('linkifies multiple URLs in one message', () => {
    const parts = toArray(
      linkifyText('a https://one.example b https://two.example c'),
    );
    const anchors = parts.filter(isAnchor);
    expect(anchors.map((a) => a.props.href)).toEqual([
      'https://one.example',
      'https://two.example',
    ]);
    expect(parts[0]).toBe('a ');
    expect(parts[parts.length - 1]).toBe(' c');
  });

  it('handles a message that is exactly a URL', () => {
    const parts = toArray(linkifyText('https://example.com'));
    expect(parts).toHaveLength(1);
    expect(isAnchor(parts[0])).toBe(true);
  });

  it('does not linkify a bare domain without a scheme', () => {
    const result = linkifyText('visit example.com for details');
    expect(result).toBe('visit example.com for details');
  });
});
