import type { ComponentChildren, VNode } from 'preact';
import { GlyphBadge } from '../../shared/GlyphBadge';

export type AnyVNode = VNode<Record<string, unknown>>;

function asHost(v: AnyVNode): AnyVNode | null {
  if (typeof v.type === 'string') return v;
  return v.type === GlyphBadge ? { ...v, type: 'span' } : null;
}

export function textOf(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(textOf).join('');
  const v = asHost(node as AnyVNode);
  if (!v) return '';
  return textOf(v.props.children as ComponentChildren);
}

export function findByClass(node: ComponentChildren, cls: string): AnyVNode[] {
  if (node === null || node === undefined || typeof node !== 'object') return [];
  if (Array.isArray(node)) return node.flatMap((n) => findByClass(n, cls));
  const v = asHost(node as AnyVNode);
  if (!v) return [];
  const out: AnyVNode[] = [];
  const klass = (v.props.class as string | undefined) ?? '';
  if (klass.split(' ').includes(cls)) out.push(v);
  return out.concat(findByClass(v.props.children as ComponentChildren, cls));
}

export function findByType(node: ComponentChildren, type: string): AnyVNode[] {
  if (node === null || node === undefined || typeof node !== 'object') return [];
  if (Array.isArray(node)) return node.flatMap((n) => findByType(n, type));
  const v = asHost(node as AnyVNode);
  if (!v) return [];
  const out: AnyVNode[] = v.type === type ? [v] : [];
  return out.concat(findByType(v.props.children as ComponentChildren, type));
}

export function componentTypes(node: ComponentChildren): unknown[] {
  if (node === null || node === undefined || typeof node !== 'object') return [];
  if (Array.isArray(node)) return node.flatMap(componentTypes);
  const v = node as AnyVNode;
  const here: unknown[] = typeof v.type === 'string' ? [] : [v.type];
  return here.concat(componentTypes(v.props.children as ComponentChildren));
}
