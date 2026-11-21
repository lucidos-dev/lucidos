import type { ComponentChildren, VNode } from 'preact';

/** Flatten a preact vnode tree to a tag-tagged string for structural
 *  assertions — no DOM, no preact-render-to-string. `class` and `disabled`
 *  are surfaced because the card tests assert on them. Function components
 *  are invoked with their props so nested helpers like OptionContent /
 *  OptionIndicator render into the flat string.
 *
 *  Because that invocation is a bare call and not a render, a component that
 *  uses HOOKS throws "Hook can only be invoked from render methods" here. Only
 *  hook-free components can be passed in: on the card surfaces that means
 *  `AnsweredBody` / `TerminatedQuestionBody` / `OptionIndicator`
 *  and the `render*Question` helpers, but NOT `QuestionBody`, `LiveOptions`,
 *  `PermissionBody`, or `PermissionBodyShell`, which hold focus-seed state.
 *  Assert on those through the browser e2e specs instead. */
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
  // dangerouslySetInnerHTML replaces children — surface its HTML so assertions
  // on markdown-rendered content (question text, option descriptions) still see
  // the text. Children are ignored by preact when this prop is set.
  const html = (v.props as { dangerouslySetInnerHTML?: { __html?: string } })
    ?.dangerouslySetInnerHTML?.__html;
  const inner = html != null ? html : vnodeToText(v.props?.children);
  return tag ? `<${tag}${cls}${dis}>${inner}</${tag}>` : inner;
}
