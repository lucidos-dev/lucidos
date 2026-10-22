import { effect } from '@preact/signals';
import { useCallback, useLayoutEffect, useRef } from 'preact/hooks';
import { useWindowDragRegion } from '../../hooks/useWindowDragRegion';
import { homeThreadId } from '../../store/actions/homeThread';
import { dashboardRailWidth, persistDashboardRailWidth } from '../../store/dashboardRail';
import { DASHBOARD_TOO_NARROW, toggleDashboardRail } from '../../store/actions/pane';
import { minDashboardRailPx, splitBounds } from '../../store/paneMinimums';
import { splitRatio, SPLIT_RATIO_KEY } from '../../store/store';
import { lazyComponent } from '../../utils/lazyComponent';
import { DashboardIcon } from '../shared/icons';
import { ToggleIconButton } from '../shared/ToggleIconButton';
import { viewportIsMobile } from '../../utils/viewport';
import { startDividerDrag } from '../layout/dividerDrag';
import { clampSplitRatio, clampToRange, dashboardRailCeiling } from '../layout/splitHelpers';
import {
  beginPaneResize, dashboardRailCanShow, drawnRailShown, drawnRailWidth, drawnSplitRatio, endPaneResize,
} from '../layout/drawnLayout';

const Dashboard = lazyComponent(() => import('./Dashboard').then((m) => m.Dashboard));

/** Set on `<html>` while the rail shows, with its width in `--dashboard-rail-width`.
 *  The shell gives up that width on its right (`styles/panels/dashboard.css`),
 *  so the header and the panes never reach under the rail. */
const SHOWN_ATTR = 'data-dashboard-rail';

/** Set on `<html>` while the rail's toggle is offered. With the rail hidden,
 *  the Canvas row keeps room for it at its trailing end. */
const TOGGLE_ATTR = 'data-dashboard-rail-toggle';

/** The desktop host of the `desktop` dashboard: a rail at the window's right
 *  edge, the full height of the window, beside the header and the panes. Its
 *  divider resizes it and its toggle hides it. A phone shows its own dashboard
 *  as a sheet instead, so the rail draws nothing there. */
export function DashboardRail() {
  if (viewportIsMobile.value) return null;
  return <Rail />;
}

/** The rail's head spans the whole bar, title-bar band included, so it is the
 *  window drag region there, bar its controls. */
function dragsWindow(target: HTMLElement): boolean {
  return !!target.closest('.dashboard-head') && !target.closest('button, a, input');
}

function Rail() {
  const shown = drawnRailShown.value;
  const railRef = useRef<HTMLElement>(null);
  useWindowDragRegion(railRef, { maximize: true, canStart: dragsWindow });

  useLayoutEffect(() => {
    document.documentElement.toggleAttribute(SHOWN_ATTR, shown);
  }, [shown]);
  // The width reaches CSS without a render, so a drag never re-renders the
  // dashboard. It is the drawn width, held to the window by the layout.
  useLayoutEffect(() => {
    const root = document.documentElement;
    const stop = effect(() => root.style.setProperty('--dashboard-rail-width', `${drawnRailWidth.value}px`));
    return () => {
      stop();
      root.removeAttribute(SHOWN_ATTR);
    };
  }, []);

  return (
    <>
      <aside ref={railRef} class="dashboard-rail" aria-label="Dashboard" aria-hidden={!shown} inert={!shown}>
        <DashboardRailDivider />
        <div class="dashboard-rail-body">
          <Dashboard dashboard="desktop" hidden={!shown} />
        </div>
      </aside>
      <DashboardRailToggle />
    </>
  );
}

/** Shows and hides the rail. One control, in the window's corner in both
 *  states: it ends the rail's head while the rail shows, and the Canvas row
 *  while it is hidden (`styles/panels/dashboard.css`). It shows once the thread
 *  list carries Home, the gate Home's entries share (ADR 0455). An empty
 *  dashboard keeps it too, so a hidden rail can always come back. It reads what
 *  is drawn, and a window too narrow for the rail disables it. */
export function DashboardRailToggle() {
  const shown = drawnRailShown.value;
  const home = homeThreadId.value;
  const offered = !!home;
  const blocked = !shown && !dashboardRailCanShow();

  useLayoutEffect(() => {
    const root = document.documentElement;
    root.toggleAttribute(TOGGLE_ATTR, offered);
    return () => root.removeAttribute(TOGGLE_ATTR);
  }, [offered]);

  if (!home) return null;
  const label = shown ? 'Hide dashboard' : 'Show dashboard';
  return (
    <div class="dashboard-rail-toggle-slot">
      <ToggleIconButton
        class="header-icon dashboard-rail-toggle"
        onClick={blocked ? undefined : toggleDashboardRail}
        aria-label={label}
        pressed={shown}
        aria-disabled={blocked || undefined}
        data-tooltip={blocked ? DASHBOARD_TOO_NARROW : label}
      >
        <DashboardIcon />
      </ToggleIconButton>
    </div>
  );
}

/** A clamped drag (ADR 0056): never narrower than two dashboard cells, and never
 *  past what the panes beside it need. It never hides the rail. */
function DashboardRailDivider() {
  const onPointerDown = useCallback((e: PointerEvent) => {
    e.preventDefault();
    const split = document.querySelector('.split-layout');
    if (!split) return;
    beginPaneResize();
    // Measured once: the floors derive from the root font size, which a drag
    // cannot change.
    const bounds = splitBounds();
    const floor = minDashboardRailPx();
    const ratio = drawnSplitRatio.peek();
    const splitPx = split.getBoundingClientRect().width;
    // The drawn width, which a smaller window may hold below the saved one.
    const railPx = (e.currentTarget as HTMLElement).parentElement?.getBoundingClientRect().width ?? dashboardRailWidth.value;
    const roomPx = splitPx + railPx;
    const threadPx = splitPx * ratio;
    // The stored ratio says which split panes the layout holds the rail for,
    // so a yielded Canvas still counts: the drag reaches no further than the drawn hold.
    const ceiling = dashboardRailCeiling(roomPx, threadPx, splitRatio.peek(), bounds);
    const onMove = (m: PointerEvent) => {
      const next = clampToRange(window.innerWidth - m.clientX, floor, ceiling);
      dashboardRailWidth.value = next;
      // The Conversation holds its width, so the Canvas takes the change.
      if (ratio > 0 && ratio < 1) splitRatio.value = clampSplitRatio(threadPx, roomPx - next, bounds);
    };
    startDividerDrag(e, onMove, () => {
      persistDashboardRailWidth();
      localStorage.setItem(SPLIT_RATIO_KEY, String(splitRatio.value));
      endPaneResize();
    });
  }, []);

  return (
    <div
      class="dashboard-rail-divider"
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize dashboard"
      onPointerDown={onPointerDown}
    />
  );
}
