import type { ComponentChildren, VNode } from 'preact';

/** Flatten a preact vnode tree to a tag-tagged string for structural
 *  assertions — no DOM, no preact-render-to-string. `class` and `disabled`
 *  are surfaced because the card tests assert on them. Function components
 *  are invoked with their props so nested helpers like ModeBadge /
 *  OptionContent render into the flat string. */
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
  const inner = vnodeToText(v.props?.children);
  return tag ? `<${tag}${cls}${dis}>${inner}</${tag}>` : inner;
}
