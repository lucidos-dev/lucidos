import { describe, it, expect } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { DropdownChevron, DropdownSkeleton } from '../Dropdown';

function collectClasses(node: ComponentChildren, out: string[] = []): string[] {
  if (node === null || node === undefined || typeof node !== 'object') return out;
  if (Array.isArray(node)) {
    for (const n of node) collectClasses(n, out);
    return out;
  }
  const v = node as VNode<{ children?: ComponentChildren; class?: string }>;
  if (v.props?.class) out.push(v.props.class);
  return collectClasses(v.props?.children, out);
}

function containsType(node: ComponentChildren, type: unknown): boolean {
  if (node === null || node === undefined || typeof node !== 'object') return false;
  if (Array.isArray(node)) return node.some(n => containsType(n, type));
  const v = node as VNode<{ children?: ComponentChildren }>;
  return v.type === type || containsType(v.props?.children, type);
}

function firstWithClass(node: ComponentChildren, cls: string): VNode<Record<string, unknown>> | null {
  if (node === null || node === undefined || typeof node !== 'object') return null;
  if (Array.isArray(node)) {
    for (const n of node) {
      const hit = firstWithClass(n, cls);
      if (hit) return hit;
    }
    return null;
  }
  const v = node as VNode<{ children?: ComponentChildren; class?: string }>;
  if (v.props?.class === cls) return v as VNode<Record<string, unknown>>;
  return firstWithClass(v.props?.children, cls);
}

describe('DropdownSkeleton', () => {
  const tree = DropdownSkeleton({ w: '5rem' });

  it('borrows the real trigger box, so the control lands in its place', () => {
    expect(collectClasses(tree)).toContain('dropdown-trigger dropdown-skeleton');
  });

  it('carries the chevron, so the skeleton is not narrower than the control by one', () => {
    expect(containsType(tree, DropdownChevron)).toBe(true);
  });

  it('is decorative: hidden from assistive tech, and not a focusable button', () => {
    const skeleton = firstWithClass(tree, 'dropdown-trigger dropdown-skeleton');
    expect(skeleton?.props['aria-hidden']).toBe('true');
    expect(skeleton?.type).toBe('span');
  });
});
