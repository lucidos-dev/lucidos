import type { ComponentChildren, VNode } from 'preact';

export function vnodeToText(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(vnodeToText).join('');
  const v = node as VNode<{ children?: ComponentChildren; class?: string; disabled?: boolean }>;
  if (typeof v.type === 'function') {
    const Fn = v.type as (props: Record<string, unknown>) => ComponentChildren;
    return vnodeToText(Fn(v.props as Record<string, unknown>));
  }
  const tag = typeof v.type === 'string' ? v.type : '';
  const cls = v.props?.class ? ` class="${v.props.class}"` : '';
  const dis = v.props?.disabled ? ' disabled' : '';
  const html = (v.props as { dangerouslySetInnerHTML?: { __html?: string } })
    ?.dangerouslySetInnerHTML?.__html;
  const inner = html != null ? html : vnodeToText(v.props?.children);
  return tag ? `<${tag}${cls}${dis}>${inner}</${tag}>` : inner;
}
