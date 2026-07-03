import { describe, it, expect } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { StoreTabSkeleton } from '../StoreTab';
import { ListSkeletonOf } from '../../shared/Skeleton';

// Walk a vnode tree WITHOUT invoking function components (their hooks would throw
// outside a real render). Record class names of host elements and which function
// components are reached by reference. StoreTabSkeleton is pure/hookless, so
// calling it as a plain function is safe; ListSkeletonOf stays an uninvoked child.
function inspect(
  node: ComponentChildren,
  acc: { classes: string[]; types: Set<unknown> },
): void {
  if (node === null || node === undefined || typeof node === 'boolean') return;
  if (typeof node === 'string' || typeof node === 'number') return;
  if (Array.isArray(node)) {
    node.forEach((n) => inspect(n, acc));
    return;
  }
  const v = node as VNode<Record<string, unknown>>;
  if (typeof v.type === 'function') {
    acc.types.add(v.type); // record but do NOT recurse/invoke
    return;
  }
  if (typeof v.props?.class === 'string') acc.classes.push(v.props.class);
  inspect(v.props?.children as ComponentChildren, acc);
}

function render() {
  const acc = { classes: [] as string[], types: new Set<unknown>() };
  inspect(StoreTabSkeleton(), acc);
  return acc;
}

describe('StoreTabSkeleton — mirrors the loaded layout so the list does not jump', () => {
  it('reserves a full (multi-line) category-pills bar above the list skeleton', () => {
    // The regression fix: the loading skeleton must include a category-filter row
    // sized to a FULLY-populated catalog (`All` + ~9 controlled-vocabulary
    // categories). A populated bar wraps to ~2 lines in a typical pane, so a
    // single line of a few pills lets the list rows jump down a line on a cold
    // reload when the real wrapping bar lands. Pin enough placeholder pills that
    // the skeleton reserves roughly the same ~2 lines.
    const { classes } = render();
    expect(classes).toContain('app-store-category-filter');
    expect(
      classes.filter((c) => c === 'app-store-category-pill-skeleton').length,
    ).toBeGreaterThanOrEqual(9);
  });

  it('renders the list skeleton (fill) below the pills placeholder', () => {
    expect(render().types.has(ListSkeletonOf)).toBe(true);
  });
});
