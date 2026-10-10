import type { ContentSize } from '@lucidos/widget-frame';
import type { JSX } from 'preact';
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import type { ThreadWidget } from '../../api/client/widgets';
import { homeThreadId } from '../../store/actions/homeThread';
import { APP_FRAME_TAP_EVENT } from '../../store/actions/widget-frame-bridge';
import { rereadThreadWidgets } from '../../store/actions/widgets';
import { showToast } from '../../store/store';
import { threadWidgetKey, threadWidgetsFor } from '../../store/widgets';
import {
  clampWindowPosition,
  clampWindowSize,
  closeWidgetWindow,
  forgetWidgetWindow,
  moveWidgetWindow,
  raiseWidgetWindow,
  resizeWidgetWindow,
  setWidgetWindowMode,
  stackRanks,
  widgetWindowLayout,
  widgetWindowLayouts,
  widgetWindows,
  WIDGET_WINDOW_MODES,
  type Box,
  type WidgetWindow,
  type WidgetWindowMode,
  type WindowSize,
} from '../../store/widgetWindows';
import { getRemPx } from '../../utils/dom';
import { scaledDurationMs } from '../../utils/motion';
import {
  CloseIcon,
  GripIcon,
  WidgetDockedModeIcon,
  WidgetMinimalModeIcon,
  WidgetWindowModeIcon,
} from '../shared/icons';
import { WidgetBar } from './WidgetCard';
import { WidgetFrame } from './WidgetFrame';
import { menuTarget, ShowInThreadButton } from './WidgetShelf';
import { useDockBox } from './useDockBox';

/** The 1x length of `widget-window-out`, which runs on `--duration-normal`. */
export const WIDGET_WINDOW_FADE_MS = 200;
const WIDGET_WINDOW_FADE_SLACK_MS = 50;
/** Pointer travel before a press becomes a drag or a resize, so a tap on a
 *  button still runs it. */
export const DRAG_THRESHOLD_PX = 4;
/** How long a tap on a touch screen keeps the floating controls up. */
export const CONTROLS_REVEAL_MS = 3000;
/** The smallest window mode size, in rem. */
const MIN_WINDOW_REM: WindowSize = { width: 12, height: 8 };
/** Window mode's first height stops at this share of the viewport. */
const WINDOW_START_HEIGHT_SHARE = 0.6;
/** How much of a minimal window must stay on screen to drag it back, in rem. */
const MINIMAL_GRAB_REM = 2;

const MODE_ACTIONS: Record<WidgetWindowMode, { label: string; icon: () => JSX.Element }> = {
  minimal: { label: 'Minimal', icon: WidgetMinimalModeIcon },
  window: { label: 'Window', icon: WidgetWindowModeIcon },
  docked: { label: 'Dock under the title', icon: WidgetDockedModeIcon },
};

/** Everything a window may cover: the whole viewport, header included. */
function windowBounds(): Box {
  return { left: 0, top: 0, right: window.innerWidth, bottom: window.innerHeight };
}

function minWindowSize(): WindowSize {
  const rem = getRemPx();
  return { width: MIN_WINDOW_REM.width * rem, height: MIN_WINDOW_REM.height * rem };
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
  const layouts = widgetWindowLayouts.value;
  const windows = all.filter((w) => w.threadId === home);

  // A widget unpinned from Home takes its window and its layout with it.
  useEffect(() => {
    if (!home || widgets?.status !== 'loaded') return;
    const stillPinned = (instanceKey: string) => widgets.data.some((p) => p.pinned && threadWidgetKey(p) === instanceKey);
    for (const w of [...all, ...layouts]) {
      if (w.threadId === home && !stillPinned(w.instanceKey)) forgetWidgetWindow(home, w.instanceKey);
    }
  }, [home, widgets, all, layouts]);

  // Open windows cannot draw without Home's widgets, so a failed read says so.
  // The next stream reconnect reads them again.
  const hiddenByFailure = widgets?.status === 'failed' && windows.length > 0 ? widgets.error : null;
  useEffect(() => {
    if (hiddenByFailure) showToast(`Couldn't read Home's widgets, so their windows are hidden: ${hiddenByFailure}`, 'error');
  }, [hiddenByFailure]);

  // Docked windows stack under the title row in the order they were opened.
  const docked = windows.filter((w) => widgetWindowLayout(w.threadId, w.instanceKey).mode === 'docked');
  const dockBox = useDockBox(docked.length > 0);
  const [dockHeights, setDockHeights] = useState<ReadonlyMap<string, number>>(new Map());
  const onDockedHeight = useCallback((instanceKey: string, px: number) => {
    setDockHeights((prev) => (prev.get(instanceKey) === px ? prev : new Map(prev).set(instanceKey, px)));
  }, []);

  if (!pinned) return null;
  const ranks = stackRanks(windows);
  const dockTops = new Map<string, number>();
  let dockTop = dockBox?.top ?? 0;
  for (const w of docked) {
    dockTops.set(w.instanceKey, dockTop);
    dockTop += dockHeights.get(w.instanceKey) ?? 0;
  }
  return (
    <>
      {windows.map((w) => {
        const entry = pinned.find((p) => threadWidgetKey(p) === w.instanceKey);
        const top = dockTops.get(w.instanceKey);
        const dock = dockBox && top !== undefined ? { left: dockBox.left, top, width: dockBox.right - dockBox.left } : null;
        return entry && (
          <WidgetWindowView
            key={w.instanceKey}
            placed={w}
            entry={entry}
            rank={ranks.get(w.instanceKey) ?? 0}
            dock={dock}
            onDockedHeight={onDockedHeight}
          />
        );
      })}
    </>
  );
}

function currentRaise({ threadId, instanceKey }: WidgetWindow): number | undefined {
  return widgetWindows.value.find((w) => w.threadId === threadId && w.instanceKey === instanceKey)?.raise;
}

interface Gesture {
  kind: 'move' | 'resize';
  pointerId: number;
  startX: number;
  startY: number;
  from: { x: number; y: number };
  fromSize: WindowSize;
  moving: boolean;
}

interface Dock {
  left: number;
  top: number;
  width: number;
}

function WidgetWindowView({ placed, entry, rank, dock, onDockedHeight }: {
  placed: WidgetWindow;
  entry: ThreadWidget;
  rank: number;
  /** Where it hangs while docked; null while the title row is off screen. */
  dock: Dock | null;
  onDockedHeight: (instanceKey: string, px: number) => void;
}) {
  const { threadId, instanceKey } = placed;
  const layout = widgetWindowLayout(threadId, instanceKey);
  const { mode } = layout;
  const ref = useRef<HTMLDivElement>(null);
  const gesture = useRef<Gesture | null>(null);
  const [dragAt, setDragAt] = useState<{ x: number; y: number } | null>(null);
  const [resizeTo, setResizeTo] = useState<WindowSize | null>(null);
  // The raise a Close was pressed at. Picking the widget again raises it,
  // which cancels a close still fading out.
  const [closedAt, setClosedAt] = useState<number | null>(null);
  const closing = closedAt !== null && closedAt === placed.raise;
  const [measured, setMeasured] = useState({ width: 0, barHeight: 0 });
  const [bounds, setBounds] = useState(windowBounds);
  const [content, setContent] = useState<ContentSize | null>(null);
  // The mode the widget last reported a size in, since a report from before a
  // mode change describes the old layout.
  const [reportedIn, setReportedIn] = useState<WidgetWindowMode | null>(null);
  const modeRef = useRef(mode);
  modeRef.current = mode;
  const [revealed, setRevealed] = useState(false);
  const hideTimer = useRef<ReturnType<typeof setTimeout>>();

  const onSize = useCallback((size: ContentSize, fromReport: boolean) => {
    setContent(size);
    if (fromReport) setReportedIn(modeRef.current);
  }, []);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return undefined;
    const measure = () => {
      const bar = el.querySelector<HTMLElement>('.widget-bar')?.offsetHeight;
      setMeasured({ width: el.offsetWidth, barHeight: bar ?? Math.min(el.offsetHeight, MINIMAL_GRAB_REM * getRemPx()) });
      if (modeRef.current === 'docked') onDockedHeight(instanceKey, el.offsetHeight);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    const onResize = () => setBounds(windowBounds());
    window.addEventListener('resize', onResize);
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', onResize);
    };
  }, [mode, instanceKey, onDockedHeight]);

  useEffect(() => {
    if (!closing) return undefined;
    const timer = setTimeout(
      () => closeWidgetWindow(threadId, instanceKey),
      scaledDurationMs(WIDGET_WINDOW_FADE_MS) + WIDGET_WINDOW_FADE_SLACK_MS,
    );
    return () => clearTimeout(timer);
  }, [closing, threadId, instanceKey]);

  // A tap on a touch screen shows the floating controls for a while. A press
  // inside the frame reaches only the frame, so the SDK passes it up.
  const reveal = useCallback(() => {
    setRevealed(true);
    clearTimeout(hideTimer.current);
    hideTimer.current = setTimeout(() => setRevealed(false), CONTROLS_REVEAL_MS);
  }, []);
  useEffect(() => () => clearTimeout(hideTimer.current), []);
  useEffect(() => {
    const el = ref.current;
    if (!el) return undefined;
    const onTap = (e: Event) => {
      if ((e as CustomEvent<string>).detail !== 'mouse') reveal();
    };
    el.addEventListener(APP_FRAME_TAP_EVENT, onTap);
    return () => el.removeEventListener(APP_FRAME_TAP_EVENT, onTap);
  }, [reveal]);
  // A press anywhere else puts them away at once. It is not a dismiss: the
  // press goes on to whatever it landed on.
  useEffect(() => {
    if (!revealed) return undefined;
    const onPress = (e: PointerEvent) => {
      if (ref.current?.contains(e.target as Node)) return;
      clearTimeout(hideTimer.current);
      setRevealed(false);
    };
    document.addEventListener('pointerdown', onPress, true);
    return () => document.removeEventListener('pointerdown', onPress, true);
  }, [revealed]);

  const at = clampWindowPosition(dragAt ?? layout, measured, bounds);
  // A size kept from a bigger screen still fits this one, corner included.
  const size = resizeTo ?? (layout.size && clampWindowSize(layout.size, at, bounds, minWindowSize()));

  // Window mode holds its size: the first report in it sets the size once,
  // so a widget growing later scrolls inside the frame instead.
  useLayoutEffect(() => {
    const el = ref.current;
    if (mode !== 'window' || layout.size || reportedIn !== 'window' || !el) return;
    const start = { width: el.offsetWidth, height: Math.min(el.offsetHeight, window.innerHeight * WINDOW_START_HEIGHT_SHARE) };
    resizeWidgetWindow(threadId, instanceKey, clampWindowSize(start, at, bounds, minWindowSize()));
  });

  const onPointerDown = (e: PointerEvent) => {
    raiseWidgetWindow(threadId, instanceKey);
    if (e.pointerType !== 'mouse' && mode !== 'window') reveal();
    const target = e.target as Element;
    if (e.button !== 0) return;
    const resizing = !!target.closest('.widget-window-resize');
    const handle = target.closest('.widget-bar, .widget-window-grip') && !target.closest('button, a, [role="button"]');
    if (!resizing && (mode === 'docked' || !handle)) return;
    const el = ref.current;
    gesture.current = {
      kind: resizing ? 'resize' : 'move',
      pointerId: e.pointerId,
      startX: e.clientX,
      startY: e.clientY,
      from: at,
      fromSize: { width: el?.offsetWidth ?? 0, height: el?.offsetHeight ?? 0 },
      moving: false,
    };
  };
  const follow = (g: Gesture, e: PointerEvent) => {
    const dx = e.clientX - g.startX;
    const dy = e.clientY - g.startY;
    if (g.kind === 'move') return { at: { x: g.from.x + dx, y: g.from.y + dy } };
    const grown = { width: g.fromSize.width + dx, height: g.fromSize.height + dy };
    return { size: clampWindowSize(grown, g.from, bounds, minWindowSize()) };
  };
  const onPointerMove = (e: PointerEvent) => {
    const g = gesture.current;
    if (!g || g.pointerId !== e.pointerId) return;
    if (!g.moving) {
      if (Math.hypot(e.clientX - g.startX, e.clientY - g.startY) < DRAG_THRESHOLD_PX) return;
      g.moving = true;
      ref.current?.setPointerCapture(e.pointerId);
      setBounds(windowBounds());
    }
    const next = follow(g, e);
    if (next.at) setDragAt(next.at);
    if (next.size) setResizeTo(next.size);
  };
  const endGesture = (e: PointerEvent) => {
    const g = gesture.current;
    if (!g || g.pointerId !== e.pointerId) return;
    gesture.current = null;
    if (!g.moving) return;
    const next = follow(g, e);
    if (next.size) {
      resizeWidgetWindow(threadId, instanceKey, next.size);
    } else if (next.at) {
      const end = clampWindowPosition(next.at, measured, bounds);
      moveWidgetWindow(threadId, instanceKey, end.x, end.y);
    }
    setDragAt(null);
    setResizeTo(null);
  };

  const name = entry.label ?? entry.name;
  const close = () => setClosedAt(currentRaise(placed) ?? placed.raise);
  const closeButton = (
    <button type="button" class="icon-btn header-icon" aria-label={`Close ${name}`} data-tooltip="Close" onClick={close}>
      <CloseIcon />
    </button>
  );
  const modeButtons = WIDGET_WINDOW_MODES.filter((m) => m !== mode).map((m) => {
    const { label, icon: Icon } = MODE_ACTIONS[m];
    return (
      <button
        key={m}
        type="button"
        class="icon-btn header-icon"
        aria-label={`${label}: ${name}`}
        data-tooltip={label}
        data-widget-window-mode={m}
        onClick={() => setWidgetWindowMode(threadId, instanceKey, m)}
      >
        <Icon />
      </button>
    );
  });

  const style: Record<string, string> = { zIndex: `calc(var(--z-widget-window) + ${rank})` };
  if (mode === 'docked') {
    if (dock) Object.assign(style, { left: `${dock.left}px`, top: `${dock.top}px`, width: `${dock.width}px` });
  } else {
    Object.assign(style, { left: `${at.x}px`, top: `${at.y}px` });
  }
  if (mode === 'window' && size) Object.assign(style, { width: `${size.width}px`, height: `${size.height}px` });
  if (content) style['--widget-content-height'] = `${content.height}px`;
  if (content?.width !== undefined) style['--widget-content-width'] = `${content.width}px`;

  const classes = [
    'widget-window',
    `is-${mode}`,
    mode === 'window' && size ? 'is-sized' : '',
    mode === 'docked' && !dock ? 'is-dock-hidden' : '',
    revealed ? 'is-revealed' : '',
    dragAt ? 'is-dragging' : '',
    resizeTo ? 'is-resizing' : '',
    closing ? 'is-closing' : '',
  ].filter(Boolean).join(' ');

  return (
    <div
      ref={ref}
      class={classes}
      role="dialog"
      aria-modal="false"
      aria-label={`${name} widget`}
      data-widget-window={instanceKey}
      style={style}
      onPointerDown={onPointerDown}
      // A press inside the widget's frame reaches only the frame, but it moves
      // focus to the iframe, and that focus bubbles here.
      onFocusIn={() => raiseWidgetWindow(threadId, instanceKey)}
      onPointerMove={onPointerMove}
      onPointerUp={endGesture}
      onPointerCancel={endGesture}
    >
      {/* One tree in every mode, so a mode change never moves the frame in the
          DOM, which would reload it. */}
      <div class="widget-card widget-card-window">
        {mode === 'window' && (
          <WidgetBar
            target={menuTarget(threadId, entry)}
            icon={entry.icon}
            leading={<ShowInThreadButton threadId={threadId} name={name} shownEventId={entry.shown_event_id} />}
            trailing={<>{modeButtons}{closeButton}</>}
          />
        )}
        <WidgetFrame appId={entry.app_id} params={entry.params} reveal={entry.reveal} place="window" mode={mode} onSize={onSize} />
      </div>
      {mode !== 'window' && (
        <div class="widget-window-controls" role="toolbar" aria-label={`${name} window controls`}>
          {mode === 'minimal' && (
            <span class="widget-window-grip" data-tooltip="Drag to move" aria-hidden="true"><GripIcon /></span>
          )}
          {modeButtons}
          {closeButton}
        </div>
      )}
      {mode === 'window' && <span class="widget-window-resize" aria-hidden="true" />}
    </div>
  );
}
