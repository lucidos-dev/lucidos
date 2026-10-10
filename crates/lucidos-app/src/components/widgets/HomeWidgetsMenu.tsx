import type { ComponentChildren } from 'preact';
import { useRef } from 'preact/hooks';
import type { ThreadWidget } from '../../api/client/widgets';
import { useLongPress, type LongPressHandlers } from '../../hooks/useLongPress';
import { homeThreadId } from '../../store/actions/homeThread';
import { focusedThreadId } from '../../store/store';
import type { Loadable } from '../../store/types';
import { threadWidgetKey, threadWidgetsFor } from '../../store/widgets';
import { openWidgetWindow } from '../../store/widgetWindows';
import { getRemPx } from '../../utils/dom';
import { viewportIsMobile } from '../../utils/viewport';
import { AppIcon } from '../shared/AppIcon';
import { OverflowMenu, type OverflowMenuContext, type OverflowMenuOpener } from '../shared/OverflowMenu';

/** The header rows a Home entry can sit in. A window opens under its row. */
const HEADER_ROWS = '.mobile-header-row, .pane-header-brand';

export const NO_HOME_WIDGETS = 'No widgets are pinned to Home.';

/** The long-press menu's rows: one per widget pinned to Home, in shelf order.
 *  Until Home's widgets land, a hidden note holds a line's height, so the
 *  menu never opens as an empty sliver. */
export function homeWidgetsMenuItems(
  { run }: OverflowMenuContext,
  widgets: Loadable<ThreadWidget[]>,
  onPick: (instanceKey: string) => void,
): ComponentChildren {
  const pinned = widgets.status === 'loaded' ? widgets.data.filter((w) => w.pinned) : [];
  return (
    <>
      {pinned.map((w) => {
        const key = threadWidgetKey(w);
        return (
          <button
            key={key}
            type="button"
            role="menuitem"
            class="thread-overflow-item"
            data-home-widget={key}
            onClick={run(() => onPick(key))}
          >
            <AppIcon appId={w.app_id} name={w.name} icon={w.icon} />
            <span class="thread-overflow-label">{w.label ?? w.name}</span>
          </button>
        );
      })}
      {(widgets.status === 'not-loaded' || widgets.status === 'loading') && (
        <div class="thread-overflow-note" aria-hidden="true" style={{ visibility: 'hidden' }}>{NO_HOME_WIDGETS}</div>
      )}
      {widgets.status === 'loaded' && pinned.length === 0 && (
        <div class="thread-overflow-note">{NO_HOME_WIDGETS}</div>
      )}
      {widgets.status === 'failed' && (
        <div class="thread-overflow-note thread-overflow-note-error" role="alert">
          Couldn't read Home's widgets: {widgets.error}
        </div>
      )}
    </>
  );
}

/** Open a Home widget instance's window under the header row `near` sits in. */
export function openHomeWidgetWindow(instanceKey: string, near: HTMLElement | null): void {
  const home = homeThreadId.value;
  if (!home) return;
  const remPx = getRemPx();
  const row = near?.closest(HEADER_ROWS)?.getBoundingClientRect();
  const gap = 0.5 * remPx;
  const at = row ? { x: row.left + gap, y: row.bottom + gap } : { x: gap, y: gap };
  openWidgetWindow(home, instanceKey, at, 1.5 * remPx);
}

/** Where a Home entry's pick opens its window, when the entry itself is not in
 *  a header row, and what else the pick does first. */
export interface HomeWidgetsPressOptions {
  /** An element in the header row the window opens under. Defaults to the
   *  entry itself. */
  near?: () => HTMLElement | null;
  /** Runs before the window opens, such as closing the menu the entry is in. */
  onPick?: () => void;
  /** The entry is the Home icon, not the Lucidos menu's row. In Home a tap on
   *  it opens the menu too, since it has nowhere to go, and the menu centres
   *  under it. The row also opens over the thread list, where a tap must still
   *  go to Home, and its menu keeps to the row's leading edge. */
  homeIcon?: boolean;
}

/** A Home entry's gesture (ADR 0362): a tap runs `onTap`, and a hold or a
 *  right-click opens the menu of widgets pinned to Home. A pick opens the
 *  widget's window and leaves the user where they are.
 *
 *  Returns the handlers for the entry's element and the menu to render beside
 *  it. The menu draws nothing in place: it is portaled while open. */
export function useHomeWidgetsPress(
  onTap: (e: MouseEvent) => void,
  { near, onPick, homeIcon = false }: HomeWidgetsPressOptions = {},
): { press: LongPressHandlers; menu: ComponentChildren } {
  const openMenu = useRef<OverflowMenuOpener | null>(null);
  const press = useLongPress(
    (el, at) => openMenu.current?.(el, viewportIsMobile.value ? undefined : at),
    (e) => {
      const inHome = homeThreadId.value !== null && focusedThreadId.value === homeThreadId.value;
      if (homeIcon && inHome) openMenu.current?.(e.currentTarget as HTMLElement);
      else onTap(e);
    },
  );
  const menu = (
    <OverflowMenu
      ariaLabel="Widgets pinned to Home"
      hostOpener={{ ref: openMenu, trigger: false, align: homeIcon ? 'center' : 'start' }}
      items={(ctx) => {
        const home = homeThreadId.value;
        if (!home) return null;
        return homeWidgetsMenuItems(ctx, threadWidgetsFor(home), (instanceKey) => {
          const at = near ? near() : ctx.anchor;
          onPick?.();
          openHomeWidgetWindow(instanceKey, at);
        });
      }}
    />
  );
  return { press, menu };
}
