import { describe, it, expect } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { highlightEllipsis } from '../highlightEllipsis';

/** Walk the rendered children into a tag-tagged string so we can assert which
 *  spans got the `.ellipsis-marker` class without booting a DOM. Mirrors the
 *  walker in permission-card.test.tsx. */
function vnodeToText(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(vnodeToText).join('');
  const v = node as VNode<{ children?: ComponentChildren; class?: string }>;
  const tag = typeof v.type === 'string' ? v.type : '';
  const cls = v.props?.class ? ` class="${v.props.class}"` : '';
  const inner = vnodeToText(v.props?.children);
  return tag ? `<${tag}${cls}>${inner}</${tag}>` : inner;
}

describe('highlightEllipsis', () => {
  it('returns plain text when there is no ellipsis to mark', () => {
    expect(highlightEllipsis('Reading file.rs')).toBe('Reading file.rs');
  });

  it('wraps a trailing "..." in an ellipsis-marker span', () => {
    const text = vnodeToText(highlightEllipsis('Grepping pattern...'));
    expect(text).toBe('Grepping pattern<span class="ellipsis-marker">...</span>');
  });

  it('wraps a middle Unicode "…" in an ellipsis-marker span', () => {
    const text = vnodeToText(highlightEllipsis("Search 'restart.*requir…d_required'"));
    expect(text).toBe(
      `Search 'restart.*requir<span class="ellipsis-marker">…</span>d_required'`,
    );
  });

  it('wraps both a middle "…" and a trailing "..."', () => {
    const text = vnodeToText(highlightEllipsis('Running: ls -la /tm…/foo...'));
    expect(text).toBe(
      'Running: ls -la /tm<span class="ellipsis-marker">…</span>/foo<span class="ellipsis-marker">...</span>',
    );
  });

  it('only treats "..." as an elision when it is at the very end', () => {
    // A literal "..." in the middle of text must stay as content — we cannot
    // distinguish it from real ASCII content there. Only the trailing form
    // (which is what `describe_tool` emits) gets marked.
    const text = vnodeToText(highlightEllipsis('Edit a...b file'));
    expect(text).toBe('Edit a...b file');
  });

  it('handles consecutive Unicode ellipses without dropping content', () => {
    const text = vnodeToText(highlightEllipsis('a…b…c'));
    expect(text).toBe(
      'a<span class="ellipsis-marker">…</span>b<span class="ellipsis-marker">…</span>c',
    );
  });
});
