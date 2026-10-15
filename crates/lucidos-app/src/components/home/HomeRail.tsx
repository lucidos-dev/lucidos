import { effect } from '@preact/signals';
import { useCallback, useLayoutEffect, useRef } from 'preact/hooks';
import { useWindowDragRegion } from '../../hooks/useWindowDragRegion';
import type { HomeBoard as HomeBoardData } from '../../api/client/widgets';
import { homeThreadId } from '../../store/actions/homeThread';
import { homeBoardFor } from '../../store/homeBoard';
import { homeRailShown, homeRailWidth, persistHomeRailWidth, toggleHomeRail } from '../../store/homeRail';
import type { Loadable } from '../../store/types';
import { minHomeRailPx, splitBounds } from '../../store/paneMinimums';
import { splitRatio, SPLIT_RATIO_KEY } from '../../store/store';
import { lazyComponent } from '../../utils/lazyComponent';
import { RightSidebarIcon } from '../shared/icons';
import { viewportIsMobile } from '../../utils/viewport';
import { startDividerDrag } from '../layout/dividerDrag';
import { beginPaneResize, clampSplitRatio, clampToRange, drawnHomeRailWidth, endPaneResize, homeRailCeiling } from '../layout/splitHelpers';

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
  const shown = homeRailShown.value;
  const railRef = useRef<HTMLElement>(null);
  useWindowDragRegion(railRef, { maximize: true, canStart: dragsWindow });

  useLayoutEffect(() => {
    document.documentElement.toggleAttribute(SHOWN_ATTR, shown);
  }, [shown]);
  // The width reaches CSS without a render, so a drag never re-renders the
  // board. A window resize draws it again, held to that window.
  useLayoutEffect(() => {
    const root = document.documentElement;
    const draw = (saved: number) => {
      const width = drawnHomeRailWidth(saved, window.innerWidth, minHomeRailPx(), splitBounds());
      root.style.setProperty('--home-rail-width', `${width}px`);
    };
    const stop = effect(() => draw(homeRailWidth.value));
    const onResize = () => draw(homeRailWidth.peek());
    window.addEventListener('resize', onResize);
    return () => {
      stop();
      window.removeEventListener('resize', onResize);
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

/** Whether the toggle is offered: always while the rail shows, so it can be
 *  hidden, and while it is hidden only if the board has a tile to show. */
export function homeRailToggleOffered(shown: boolean, board: Loadable<HomeBoardData>): boolean {
  return shown || (board.status === 'loaded' && board.data.tiles.length > 0);
}

/** Shows and hides the rail. One control, in the window's corner in both
 *  states: it ends the rail's head while the rail shows, and the Canvas row
 *  while it is hidden (`styles/panels/home-board.css`). Not offered, it
 *  shrinks away in place rather than unmounting. */
export function HomeRailToggle() {
  const shown = homeRailShown.value;
  const home = homeThreadId.value;
  const offered = !!home && homeRailToggleOffered(shown, homeBoardFor('desktop'));

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
        onClick={toggleHomeRail}
        aria-label={label}
        aria-pressed={shown}
        data-tooltip={label}
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
    const ratio = splitRatio.value;
    const splitPx = split.getBoundingClientRect().width;
    // The drawn width, which a smaller window may hold below the saved one.
    const railPx = (e.currentTarget as HTMLElement).parentElement?.getBoundingClientRect().width ?? homeRailWidth.value;
    const roomPx = splitPx + railPx;
    const threadPx = splitPx * ratio;
    const ceiling = homeRailCeiling(roomPx, threadPx, ratio, bounds);
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
