import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import type { ThreadWidget } from '../../api/client/widgets';
import { homeThreadId } from '../../store/actions/homeThread';
import { rereadThreadWidgets } from '../../store/actions/widgets';
import { showToast } from '../../store/store';
import { threadWidgetKey, threadWidgetsFor } from '../../store/widgets';
import {
  clampWindowPosition,
  closeWidgetWindow,
  moveWidgetWindow,
  raiseWidgetWindow,
  stackRanks,
  widgetWindows,
  type Box,
  type WidgetWindow,
} from '../../store/widgetWindows';
import { scaledDurationMs } from '../../utils/motion';
import { OpenShelfWidget } from './WidgetShelf';

/** The 1x length of `widget-window-out`, which runs on `--duration-normal`. */
export const WIDGET_WINDOW_FADE_MS = 200;
const WIDGET_WINDOW_FADE_SLACK_MS = 50;
/** Pointer travel on the bar before a press becomes a drag, so a tap on one of
 *  the bar's buttons still runs it. */
export const DRAG_THRESHOLD_PX = 4;

/** Everything a window may cover: the viewport below the app header. */
function windowBounds(): Box {
  const header = document.querySelector('.app-header')?.getBoundingClientRect().bottom ?? 0;
  return { left: 0, top: header, right: window.innerWidth, bottom: window.innerHeight };
}

/** Home's widget windows (ADR 0419), mounted once in the overlay layer. Unlike
 *  every overlay, a window is not an `<Overlay>`: it stays open while the user
 *  works, and only its own Close shuts it.
 *
 *  It also starts the first read of Home's widgets, so SSE keeps them fresh for
 *  the windows and the Home long-press menu from any thread. */
export function WidgetWindows() {
  const home = homeThreadId.value;
  useEffect(() => {
    if (home && threadWidgetsFor(home).status === 'not-loaded') rereadThreadWidgets(home);
  }, [home]);

  const widgets = home ? threadWidgetsFor(home) : null;
  const pinned = widgets?.status === 'loaded' ? widgets.data.filter((w) => w.pinned) : null;
  const all = widgetWindows.value;
  const windows = all.filter((w) => w.threadId === home);

  // A widget unpinned from Home takes its window with it.
  useEffect(() => {
    if (!home || widgets?.status !== 'loaded') return;
    for (const w of all) {
      const stillPinned = widgets.data.some((p) => p.pinned && threadWidgetKey(p) === w.instanceKey);
      if (w.threadId === home && !stillPinned) closeWidgetWindow(home, w.instanceKey);
    }
  }, [home, widgets, all]);

  // Open windows cannot draw without Home's widgets, so a failed read says so.
  // The next stream reconnect reads them again.
  const hiddenByFailure = widgets?.status === 'failed' && windows.length > 0 ? widgets.error : null;
  useEffect(() => {
    if (hiddenByFailure) showToast(`Couldn't read Home's widgets, so their windows are hidden: ${hiddenByFailure}`, 'error');
  }, [hiddenByFailure]);

  if (!pinned) return null;
  const ranks = stackRanks(windows);
  return (
    <>
      {windows.map((w) => {
        const entry = pinned.find((p) => threadWidgetKey(p) === w.instanceKey);
        return entry && <WidgetWindowView key={w.instanceKey} placed={w} entry={entry} rank={ranks.get(w.instanceKey) ?? 0} />;
      })}
    </>
  );
}

function currentRaise({ threadId, instanceKey }: WidgetWindow): number | undefined {
  return widgetWindows.value.find((w) => w.threadId === threadId && w.instanceKey === instanceKey)?.raise;
}

interface Drag {
  pointerId: number;
  startX: number;
  startY: number;
  from: { x: number; y: number };
  moving: boolean;
}

function WidgetWindowView({ placed, entry, rank }: { placed: WidgetWindow; entry: ThreadWidget; rank: number }) {
  const ref = useRef<HTMLDivElement>(null);
  const drag = useRef<Drag | null>(null);
  const [dragAt, setDragAt] = useState<{ x: number; y: number } | null>(null);
  // The raise a Close was pressed at. Picking the widget again raises it,
  // which cancels a close still fading out.
  const [closedAt, setClosedAt] = useState<number | null>(null);
  const closing = closedAt !== null && closedAt === placed.raise;
  const [size, setSize] = useState({ width: 0, barHeight: 0 });
  const [bounds, setBounds] = useState(windowBounds);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return undefined;
    const measure = () => setSize({
      width: el.offsetWidth,
      barHeight: el.querySelector<HTMLElement>('.widget-bar')?.offsetHeight ?? 0,
    });
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    const onResize = () => setBounds(windowBounds());
    window.addEventListener('resize', onResize);
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', onResize);
    };
  }, []);

  useEffect(() => {
    if (!closing) return undefined;
    const timer = setTimeout(
      () => closeWidgetWindow(placed.threadId, placed.instanceKey),
      scaledDurationMs(WIDGET_WINDOW_FADE_MS) + WIDGET_WINDOW_FADE_SLACK_MS,
    );
    return () => clearTimeout(timer);
  }, [closing, placed.threadId, placed.instanceKey]);

  const at = clampWindowPosition(dragAt ?? placed, size, bounds);

  const onPointerDown = (e: PointerEvent) => {
    raiseWidgetWindow(placed.threadId, placed.instanceKey);
    const target = e.target as Element;
    if (e.button !== 0 || !target.closest('.widget-bar') || target.closest('button, a, [role="button"]')) return;
    drag.current = { pointerId: e.pointerId, startX: e.clientX, startY: e.clientY, from: at, moving: false };
  };
  const onPointerMove = (e: PointerEvent) => {
    const d = drag.current;
    if (!d || d.pointerId !== e.pointerId) return;
    const dx = e.clientX - d.startX;
    const dy = e.clientY - d.startY;
    if (!d.moving) {
      if (Math.hypot(dx, dy) < DRAG_THRESHOLD_PX) return;
      d.moving = true;
      ref.current?.setPointerCapture(e.pointerId);
      setBounds(windowBounds());
    }
    setDragAt({ x: d.from.x + dx, y: d.from.y + dy });
  };
  const endDrag = (e: PointerEvent) => {
    const d = drag.current;
    if (!d || d.pointerId !== e.pointerId) return;
    drag.current = null;
    if (!d.moving) return;
    const end = clampWindowPosition({ x: d.from.x + e.clientX - d.startX, y: d.from.y + e.clientY - d.startY }, size, bounds);
    moveWidgetWindow(placed.threadId, placed.instanceKey, end.x, end.y);
    setDragAt(null);
  };

  return (
    <div
      ref={ref}
      class={`widget-window${dragAt ? ' is-dragging' : ''}${closing ? ' is-closing' : ''}`}
      role="dialog"
      aria-modal="false"
      aria-label={`${entry.label ?? entry.name} widget`}
      data-widget-window={placed.instanceKey}
      style={{ left: `${at.x}px`, top: `${at.y}px`, zIndex: `calc(var(--z-widget-window) + ${rank})` }}
      onPointerDown={onPointerDown}
      // A press inside the widget's frame reaches only the frame, but it moves
      // focus to the iframe, and that focus bubbles here.
      onFocusIn={() => raiseWidgetWindow(placed.threadId, placed.instanceKey)}
      onPointerMove={onPointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
    >
      <OpenShelfWidget
        threadId={placed.threadId}
        entry={entry}
        // Read live: the press that reached Close may have just raised it.
        onClose={() => setClosedAt(currentRaise(placed) ?? placed.raise)}
      />
    </div>
  );
}
