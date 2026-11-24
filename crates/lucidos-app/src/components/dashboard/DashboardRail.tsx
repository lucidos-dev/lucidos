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

const SHOWN_ATTR = 'data-dashboard-rail';

const TOGGLE_ATTR = 'data-dashboard-rail-toggle';

export function DashboardRail() {
  if (viewportIsMobile.value) return null;
  return <Rail />;
}

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
        tone="quiet"
        aria-disabled={blocked || undefined}
        data-tooltip={blocked ? DASHBOARD_TOO_NARROW : label}
      >
        <DashboardIcon />
      </ToggleIconButton>
    </div>
  );
}

function DashboardRailDivider() {
  const onPointerDown = useCallback((e: PointerEvent) => {
    e.preventDefault();
    const split = document.querySelector('.split-layout');
    if (!split) return;
    beginPaneResize();
    const bounds = splitBounds();
    const floor = minDashboardRailPx();
    const ratio = drawnSplitRatio.peek();
    const splitPx = split.getBoundingClientRect().width;
    const railPx = (e.currentTarget as HTMLElement).parentElement?.getBoundingClientRect().width ?? dashboardRailWidth.value;
    const roomPx = splitPx + railPx;
    const threadPx = splitPx * ratio;
    const ceiling = dashboardRailCeiling(roomPx, threadPx, splitRatio.peek(), bounds);
    const onMove = (m: PointerEvent) => {
      const next = clampToRange(window.innerWidth - m.clientX, floor, ceiling);
      dashboardRailWidth.value = next;
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
