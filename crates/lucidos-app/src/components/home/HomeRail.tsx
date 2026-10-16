import { effect } from '@preact/signals';
import { useCallback, useLayoutEffect, useRef } from 'preact/hooks';
import { useWindowDragRegion } from '../../hooks/useWindowDragRegion';
import { homeThreadId } from '../../store/actions/homeThread';
import { homeRailWidth, persistHomeRailWidth } from '../../store/homeRail';
import { HOME_BOARD_TOO_NARROW, toggleHomeRail } from '../../store/actions/pane';
import { minHomeRailPx, splitBounds } from '../../store/paneMinimums';
import { splitRatio, SPLIT_RATIO_KEY } from '../../store/store';
import { lazyComponent } from '../../utils/lazyComponent';
import { RightSidebarIcon } from '../shared/icons';
import { viewportIsMobile } from '../../utils/viewport';
import { startDividerDrag } from '../layout/dividerDrag';
import { clampSplitRatio, clampToRange, homeRailCeiling } from '../layout/splitHelpers';
import {
  beginPaneResize, drawnRailShown, drawnRailWidth, drawnSplitRatio, endPaneResize, homeRailCanShow,
} from '../layout/drawnLayout';

const HomeBoard = lazyComponent(() => import('./HomeBoard').then((m) => m.HomeBoard));

/** Set on `<html>` while the rail shows, with its width in `--home-rail-width`.
 *  The shell gives up that width on its right (`styles/panels/home-board.css`),
 *  so the header and the panes never reach under the rail. */
const SHOWN_ATTR = 'data-home-rail';

/** Set on `<html>` while the rail's toggle is offered. With the rail hidden,
 *  the Canvas row keeps room for it at its trailing end. */
const TOGGLE_ATTR = 'data-home-rail-toggle';

/** The desktop host of the Home board: a rail at the window's right edge, the
 *  full height of the window, beside the header and the panes. Its divider
 *  resizes it and its toggle hides it. A phone shows the board as Home's
 *  face instead, so it draws nothing there. */
export function HomeRail() {
  if (viewportIsMobile.value) return null;
  return <Rail />;
}

/** The rail's head spans the whole bar, title-bar band included, so it is the
 *  window drag region there, bar its controls. */
function dragsWindow(target: HTMLElement): boolean {
  return !!target.closest('.home-board-head') && !target.closest('button, a, input');
}

function Rail() {
  const shown = drawnRailShown.value;
  const railRef = useRef<HTMLElement>(null);
  useWindowDragRegion(railRef, { maximize: true, canStart: dragsWindow });

  useLayoutEffect(() => {
    document.documentElement.toggleAttribute(SHOWN_ATTR, shown);
  }, [shown]);
  // The width reaches CSS without a render, so a drag never re-renders the
  // board. It is the drawn width, held to the window by the layout.
  useLayoutEffect(() => {
    const root = document.documentElement;
    const stop = effect(() => root.style.setProperty('--home-rail-width', `${drawnRailWidth.value}px`));
    return () => {
      stop();
      root.removeAttribute(SHOWN_ATTR);
    };
  }, []);

  return (
    <>
      <aside ref={railRef} class="home-rail" aria-label="Home board" aria-hidden={!shown} inert={!shown}>
        <HomeRailDivider />
        <div class="home-rail-body">
          <HomeBoard layout="desktop" hidden={!shown} />
        </div>
      </aside>
      <HomeRailToggle />
    </>
  );
}

/** Shows and hides the rail. One control, in the window's corner in both
 *  states: it ends the rail's head while the rail shows, and the Canvas row
 *  while it is hidden (`styles/panels/home-board.css`). Offered whenever Home
 *  exists, an empty board included, so a hidden rail can always come back. It
 *  reads what is drawn, and a window too narrow for the rail disables it. */
export function HomeRailToggle() {
  const shown = drawnRailShown.value;
  const home = homeThreadId.value;
  const offered = !!home;
  const blocked = !shown && !homeRailCanShow();

  useLayoutEffect(() => {
    const root = document.documentElement;
    root.toggleAttribute(TOGGLE_ATTR, offered);
    return () => root.removeAttribute(TOGGLE_ATTR);
  }, [offered]);

  if (!home) return null;
  const label = shown ? 'Hide Home board' : 'Show Home board';
  return (
    <div class="home-rail-toggle-slot">
      <button
        type="button"
        class="icon-btn header-icon home-rail-toggle"
        onClick={blocked ? undefined : toggleHomeRail}
        aria-label={label}
        aria-pressed={shown}
        aria-disabled={blocked || undefined}
        data-tooltip={blocked ? HOME_BOARD_TOO_NARROW : label}
      >
        <RightSidebarIcon />
      </button>
    </div>
  );
}

/** A clamped drag (ADR 0056): never narrower than two board cells, and never
 *  past what the panes beside it need. It never hides the rail. */
function HomeRailDivider() {
  const onPointerDown = useCallback((e: PointerEvent) => {
    e.preventDefault();
    const split = document.querySelector('.split-layout');
    if (!split) return;
    beginPaneResize();
    // Measured once: the floors derive from the root font size, which a drag
    // cannot change.
    const bounds = splitBounds();
    const floor = minHomeRailPx();
    const ratio = drawnSplitRatio.peek();
    const splitPx = split.getBoundingClientRect().width;
    // The drawn width, which a smaller window may hold below the saved one.
    const railPx = (e.currentTarget as HTMLElement).parentElement?.getBoundingClientRect().width ?? homeRailWidth.value;
    const roomPx = splitPx + railPx;
    const threadPx = splitPx * ratio;
    // The stored ratio says which split panes the layout holds the rail for,
    // so a yielded Canvas still counts: the drag reaches no further than the drawn hold.
    const ceiling = homeRailCeiling(roomPx, threadPx, splitRatio.peek(), bounds);
    const onMove = (m: PointerEvent) => {
      const next = clampToRange(window.innerWidth - m.clientX, floor, ceiling);
      homeRailWidth.value = next;
      // The Conversation holds its width, so the Canvas takes the change.
      if (ratio > 0 && ratio < 1) splitRatio.value = clampSplitRatio(threadPx, roomPx - next, bounds);
    };
    startDividerDrag(e, onMove, () => {
      persistHomeRailWidth();
      localStorage.setItem(SPLIT_RATIO_KEY, String(splitRatio.value));
      endPaneResize();
    });
  }, []);

  return (
    <div
      class="home-rail-divider"
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize Home board"
      onPointerDown={onPointerDown}
    />
  );
}
