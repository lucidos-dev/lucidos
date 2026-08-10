import { describe, it, expect } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { DropdownSkeleton } from '../Dropdown';

/** The one property that makes this skeleton worth having: it wears the REAL
 *  trigger's box, so its height comes from the same CSS rule as the control that
 *  replaces it and the row cannot jump on settle. A hand-sized `SkBlock` would
 *  pass an eyeball review and drift the next time anyone re-pads the trigger,
 *  which is exactly what this pins. No jsdom in the test infra, so this walks
 *  the returned vnode tree rather than rendering it.
 */
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
    // The trigger is a flex row of label + chevron with a gap between them.
    // Drop the chevron and every slow-loading dropdown grows on settle, which
    // is the exact layout shift the skeleton exists to prevent.
    expect(collectClasses(tree)).toContain('dropdown-chevron');
  });

  it('is decorative: hidden from assistive tech, and not a focusable button', () => {
    const skeleton = firstWithClass(tree, 'dropdown-trigger dropdown-skeleton');
    expect(skeleton?.props['aria-hidden']).toBe('true');
    expect(skeleton?.type).toBe('span');
  });
});
