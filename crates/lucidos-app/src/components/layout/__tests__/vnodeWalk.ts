/**
 * Structural walkers over a preact vnode tree, for tests that invoke a hook-free
 * component as a plain function and assert on what it returned.
 *
 * The sibling helper is `components/chat/__tests__/vnodeToText`, which flattens
 * a tree to a string. Reach for that one when the assertion is about TEXT or
 * about which tags and classes appear; reach for these when it is about a
 * specific node's PROPS (`role`, `aria-hidden`, a `data-` attribute), which a
 * flattened string cannot carry.
 *
 * Neither descends into function components: calling one that uses hooks throws
 * outside a real render, which is why the surfaces these test keep their markup
 * in hook-free `*Body` functions.
 */
import type { ComponentChildren, VNode } from 'preact';

export type AnyVNode = VNode<Record<string, unknown>>;

/** Plain-text content of a vnode subtree (host nodes only). */
export function textOf(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(textOf).join('');
  const v = node as AnyVNode;
  if (typeof v.type !== 'string') return '';
  return textOf(v.props.children as ComponentChildren);
}

/** Host vnodes whose class list includes `cls`. */
export function findByClass(node: ComponentChildren, cls: string): AnyVNode[] {
  if (node === null || node === undefined || typeof node !== 'object') return [];
  if (Array.isArray(node)) return node.flatMap((n) => findByClass(n, cls));
  const v = node as AnyVNode;
  if (typeof v.type !== 'string') return [];
  const out: AnyVNode[] = [];
  const klass = (v.props.class as string | undefined) ?? '';
  if (klass.split(' ').includes(cls)) out.push(v);
  return out.concat(findByClass(v.props.children as ComponentChildren, cls));
}

/** Every host vnode of a given tag anywhere in the tree. */
export function findByType(node: ComponentChildren, type: string): AnyVNode[] {
  if (node === null || node === undefined || typeof node !== 'object') return [];
  if (Array.isArray(node)) return node.flatMap((n) => findByType(n, type));
  const v = node as AnyVNode;
  if (typeof v.type !== 'string') return [];
  const out: AnyVNode[] = v.type === type ? [v] : [];
  return out.concat(findByType(v.props.children as ComponentChildren, type));
}
