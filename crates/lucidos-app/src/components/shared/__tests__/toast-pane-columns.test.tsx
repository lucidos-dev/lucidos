import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { ToastList } from '../Toast';
import { toasts, showToast, focusedPane, splitRatio, engineRestarting } from '../../../store/store';
import { viewportIsMobile } from '../../../utils/viewport';

/**
 * Regression: "a toast in the thread pane lowers the toasts in the content pane
 * and vice versa". Every toast shared ONE flex column, so the per-pane pin
 * (`data-toast-pane`, frozen in `showToast`) only shifted a toast sideways: it
 * still consumed a row of the shared stack and pushed the other pane's toasts
 * down. Each visible pane now owns a `.toast-column`, so a toast can only ever
 * displace toasts of its own pane.
 */

/** Walk a vnode tree, returning every node whose class list contains `cls`. */
function findByClass(node: ComponentChildren, cls: string, out: VNode[] = []): VNode[] {
  if (node === null || node === undefined || typeof node === 'boolean') return out;
  if (typeof node === 'string' || typeof node === 'number') return out;
  if (Array.isArray(node)) {
    for (const child of node) findByClass(child, cls, out);
    return out;
  }
  const v = node as VNode<{ class?: string; children?: ComponentChildren }>;
  const classAttr = v.props?.class ?? '';
  if (typeof classAttr === 'string' && classAttr.split(/\s+/).includes(cls)) out.push(v);
  findByClass(v.props?.children, cls, out);
  return out;
}

function columns(): { pane: string | undefined; toastIds: number[] }[] {
  const tree = ToastList();
  return findByClass(tree, 'toast-column').map((col) => ({
    pane: (col.props as { 'data-toast-pane'?: string })['data-toast-pane'],
    toastIds: findByClass(col.props?.children, 'toast').map(
      (t) => (t.props as unknown as { 'data-toast-id': number })['data-toast-id'],
    ),
  }));
}

/** Raise a toast as if the given pane were focused, and return its id. */
function toastFrom(pane: 'thread' | 'content', message: string): number {
  focusedPane.value = pane;
  showToast(message);
  return toasts.value[0].id;
}

beforeEach(() => {
  toasts.value = [];
  engineRestarting.value = false;
  viewportIsMobile.value = false;
  splitRatio.value = 0.4;
});

afterEach(() => {
  toasts.value = [];
  focusedPane.value = 'thread';
  splitRatio.value = 0.4;
  viewportIsMobile.value = false;
});

describe('toast stack columns', () => {
  it('stacks each pane independently while the split shows both', () => {
    const first = toastFrom('thread', 'thread toast');
    const second = toastFrom('content', 'content toast');
    const third = toastFrom('content', 'second content toast');

    expect(columns()).toEqual([
      { pane: 'thread', toastIds: [first] },
      { pane: 'content', toastIds: [third, second] },
    ]);
  });

  it('keeps the other pane mounted and empty rather than re-parenting toasts', () => {
    const id = toastFrom('thread', 'thread toast');
    expect(columns()).toEqual([
      { pane: 'thread', toastIds: [id] },
      { pane: 'content', toastIds: [] },
    ]);
  });

  it('merges into one newest-first column on mobile (one pane fills the screen)', () => {
    viewportIsMobile.value = true;
    const first = toastFrom('thread', 'thread toast');
    const second = toastFrom('content', 'content toast');

    expect(columns()).toEqual([{ pane: undefined, toastIds: [second, first] }]);
  });

  it('merges over the surviving pane when the other one is collapsed', () => {
    const first = toastFrom('thread', 'thread toast');
    const second = toastFrom('content', 'content toast');

    splitRatio.value = 1; // content pane collapsed
    expect(columns()).toEqual([{ pane: 'thread', toastIds: [second, first] }]);

    splitRatio.value = 0; // thread pane collapsed
    expect(columns()).toEqual([{ pane: 'content', toastIds: [second, first] }]);
  });

  it('renders nothing at all with no toasts', () => {
    expect(ToastList()).toBeNull();
  });
});
