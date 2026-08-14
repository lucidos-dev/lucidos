import type { ToastPlacement } from '../../store/store';

/** Pure grouping behind the toast stack's per-pane columns.
 *
 *  Per-pane is one of several shapes under comparison right now, and every
 *  other one is a single stack (`ToastPlacement`, docs/temporary-measures.md).
 *  What follows describes the per-pane shape, which `toastLayout` reaches only
 *  when that is the placement in force.
 *
 *  Toasts are pinned to the pane that was focused when they appeared
 *  (`ToastItem.pane`, frozen in `showToast`). Rendering them all into ONE
 *  column made that pin horizontal only: a toast born in the thread pane still
 *  occupied a row of the shared column, so it pushed the content pane's toasts
 *  down (and vice versa). Each pane therefore gets its own stack, and a pane's
 *  toasts only ever displace that pane's other toasts.
 *
 *  Splitting is only meaningful while both panes are actually on screen. Mobile
 *  shows one pane at a time, and a collapsed split leaves a single surviving
 *  pane; in both cases every toast goes into ONE column, which keeps them in
 *  strict newest-first order rather than segregating them by a pane the user
 *  cannot see. */
export type ToastPane = 'thread' | 'content';

/** Which panes the toast stack has to lay out over, this render.
 *   - `split`: both panes visible, one column each.
 *   - `thread-only` / `content-only`: desktop with the other pane collapsed,
 *     so one column, positioned over the surviving pane.
 *   - `single`: mobile, one column spanning the viewport. */
export type ToastLayout = 'split' | 'thread-only' | 'content-only' | 'single';

export interface ToastColumn<T> {
  /** Pane this column is positioned over, or `null` for the mobile column that
   *  spans the whole viewport. Drives `data-toast-pane` on the column element. */
  pane: ToastPane | null;
  items: T[];
}

/** Desktop pane layout for a split ratio, matching `SplitLayout`'s own
 *  `threadCollapsed` / `contentCollapsed` derivation (and therefore the
 *  `data-thread-collapsed` / `data-content-collapsed` attributes) exactly, so
 *  the columns and the panes agree on what is visible. Mid-drag the ratio is
 *  held strictly inside (0, 1), so a drag never flips the layout: the same
 *  deferred-snap contract the header regions follow.
 *
 *  Every CROSS-PANE placement answers `single`, the one column that already
 *  exists for mobile. Those shapes differ only in where the column sits and how
 *  a toast in it is drawn, which is CSS keyed on `data-toast-placement`. So the
 *  split lives in one branch here rather than in a parallel layout enum. */
export function toastLayout(
  isMobile: boolean,
  splitRatio: number,
  placement: ToastPlacement = 'pane',
): ToastLayout {
  if (placement !== 'pane') return 'single';
  if (isMobile) return 'single';
  if (splitRatio <= 0) return 'content-only';
  if (splitRatio >= 1) return 'thread-only';
  return 'split';
}

/** Group toasts into the columns `toastLayout` calls for, newest-first within
 *  each column (the input is already newest-first, since `showToast` prepends).
 *  A split renders BOTH columns even when one is empty: an empty flex column
 *  has no size, and keeping them stable means a toast is never re-parented just
 *  because its pane's stack emptied out. */
export function toastColumns<T extends { pane?: ToastPane }>(
  items: readonly T[],
  layout: ToastLayout,
): ToastColumn<T>[] {
  switch (layout) {
    case 'single':
      return [{ pane: null, items: [...items] }];
    case 'thread-only':
      return [{ pane: 'thread', items: [...items] }];
    case 'content-only':
      return [{ pane: 'content', items: [...items] }];
    case 'split':
      return [
        // A toast with no pane (nothing in the app leaves it unset today, but
        // the field is optional) falls to the thread column, the primary work
        // area, which is the same default `showToast` freezes for a non-content
        // focus.
        { pane: 'thread', items: items.filter((t) => t.pane !== 'content') },
        { pane: 'content', items: items.filter((t) => t.pane === 'content') },
      ];
  }
}
