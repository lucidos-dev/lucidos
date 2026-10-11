import { WIDGET_DRAG_THRESHOLD_PX, type ContentSize, type WidgetDrag } from '@lucidos/widget-frame';
import type { JSX } from 'preact';
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import type { ThreadWidget } from '../../api/client/widgets';
import { homeThreadId } from '../../store/actions/homeThread';
import { APP_FRAME_DRAG_EVENT, APP_FRAME_TAP_EVENT } from '../../store/actions/widget-frame-bridge';
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
  resizeFromCorner,
  resizeWidgetWindow,
  setWidgetWindowMode,
  stackRanks,
  widgetWindowLayout,
  widgetWindowLayouts,
  widgetWindows,
  RESIZE_CORNERS,
  WIDGET_WINDOW_MODES,
  type Box,
  type ResizeCorner,
  type WidgetWindow,
  type WidgetWindowMode,
  type WindowSize,
} from '../../store/widgetWindows';
import { getRemPx } from '../../utils/dom';
import { scaledDurationMs } from '../../utils/motion';
import { OverflowMenu } from '../shared/OverflowMenu';
import {
  CheckIcon,
  CloseIcon,
  WidgetDockedModeIcon,
  WidgetMinimalModeIcon,
  WidgetWindowModeIcon,
} from '../shared/icons';
import { WidgetBar } from './WidgetCard';
import { WidgetFrame } from './WidgetFrame';
import { widgetMenuItems } from './WidgetMenu';
import { menuTarget } from './WidgetShelf';
import { useDockBox } from './useDockBox';

/** The 1x length of `widget-window-out`, which runs on `--duration-normal`. */
export const WIDGET_WINDOW_FADE_MS = 200;
const WIDGET_WINDOW_FADE_SLACK_MS = 50;
/** Stands in for a pointer id on a drag the widget's frame passes up. That
 *  pointer lives in the frame, so the host never sees its events. */
const FRAME_DRAG_POINTER = -1;
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

/** The room the floating controls take above a window, gap included, in rem. */
export const CONTROLS_ROOM_REM = 2.5;

/** Whether the floating controls hang under the widget rather than over it.
 *  They never cover the widget, so they go under when there is no room above,
 *  and always when docked, where above is the title row. */
export function controlsBelow(mode: WidgetWindowMode, top: number): boolean {
  return mode === 'docked' || top < CONTROLS_ROOM_REM * getRemPx();
}

/** The mode button: the current mode's icon, opening a menu of all three. */
function ModeMenu({ mode, onPick }: { mode: WidgetWindowMode; onPick: (mode: WidgetWindowMode) => void }) {
  const { label, icon: Icon } = MODE_ACTIONS[mode];
  return (
    <OverflowMenu
      ariaLabel="Window mode"
      face={{ class: 'icon-btn header-icon widget-window-mode', label: `Mode: ${label}`, children: <Icon /> }}
      items={({ run }) => WIDGET_WINDOW_MODES.map((m) => {
        const { label: itemLabel, icon: ItemIcon } = MODE_ACTIONS[m];
        return (
          <button
            key={m}
            type="button"
            role="menuitemradio"
            aria-checked={m === mode}
            data-widget-window-mode={m}
            class="thread-overflow-item widget-mode-item"
            onClick={run(() => onPick(m))}
          >
            <ItemIcon />
            <span>{itemLabel}</span>
            {m === mode && <CheckIcon className="widget-mode-check" />}
          </button>
        );
      })}
    />
  );
}

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
  /** The corner a resize grabbed. */
  corner?: ResizeCorner;
  pointerId: number;
  startX: number;
  startY: number;
  /** Where the pointer last really was, which a cancelled gesture ends at. */
  last: { x: number; y: number };
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

  // Docked hangs under the title row, so only the other modes move.
  const movable = mode !== 'docked';

  const onPointerDown = (e: PointerEvent) => {
    raiseWidgetWindow(threadId, instanceKey);
    if (e.pointerType !== 'mouse' && mode !== 'window') reveal();
    const target = e.target as Element;
    if (e.button !== 0) return;
    const corner = target.closest<HTMLElement>('.widget-window-resize')?.dataset.corner as ResizeCorner | undefined;
    const resizing = corner !== undefined;
    const handle = target.closest('.widget-bar, .widget-window-controls') && !target.closest('button, a, [role="button"]');
    if (!resizing && (!movable || !handle)) return;
    const el = ref.current;
    gesture.current = {
      kind: resizing ? 'resize' : 'move',
      corner,
      pointerId: e.pointerId,
      startX: e.clientX,
      startY: e.clientY,
      last: { x: e.clientX, y: e.clientY },
      from: at,
      fromSize: size ?? { width: el?.offsetWidth ?? 0, height: el?.offsetHeight ?? 0 },
      moving: false,
    };
  };
  const follow = (g: Gesture, x: number, y: number): { at: { x: number; y: number }; size?: WindowSize } => {
    const dx = x - g.startX;
    const dy = y - g.startY;
    if (g.kind === 'move' || !g.corner) return { at: { x: g.from.x + dx, y: g.from.y + dy } };
    return resizeFromCorner(g.corner, g.from, g.fromSize, dx, dy, bounds, minWindowSize());
  };
  const track = (g: Gesture, x: number, y: number) => {
    g.last = { x, y };
    const next = follow(g, x, y);
    if (next.at) setDragAt(next.at);
    if (next.size) setResizeTo(next.size);
  };
  const finish = (g: Gesture, x: number, y: number) => {
    gesture.current = null;
    if (!g.moving) return;
    const next = follow(g, x, y);
    if (next.size) {
      // A corner resize already stays inside the screen, its place included.
      resizeWidgetWindow(threadId, instanceKey, next.size);
      moveWidgetWindow(threadId, instanceKey, next.at.x, next.at.y);
    } else {
      const end = clampWindowPosition(next.at, measured, bounds);
      moveWidgetWindow(threadId, instanceKey, end.x, end.y);
    }
    setDragAt(null);
    setResizeTo(null);
  };
  const onPointerMove = (e: PointerEvent) => {
    const g = gesture.current;
    if (!g || g.pointerId !== e.pointerId) return;
    if (!g.moving) {
      if (Math.hypot(e.clientX - g.startX, e.clientY - g.startY) < WIDGET_DRAG_THRESHOLD_PX) return;
      g.moving = true;
      ref.current?.setPointerCapture(e.pointerId);
      setBounds(windowBounds());
    }
    track(g, e.clientX, e.clientY);
  };
  const endGesture = (e: PointerEvent) => {
    const g = gesture.current;
    if (g && g.pointerId === e.pointerId) finish(g, e.clientX, e.clientY);
  };
  // A cancel's own coordinates can be anything, often zero.
  const cancelGesture = (e: PointerEvent) => {
    const g = gesture.current;
    if (g && g.pointerId === e.pointerId) finish(g, g.last.x, g.last.y);
  };

  // A drag that starts inside the widget arrives from its frame in screen
  // pixels. Their deltas equal client deltas, so it rides the same gesture.
  // The frame already waited out the threshold.
  const onFrameDrag = (drag: WidgetDrag) => {
    const g = gesture.current;
    if (drag.phase === 'start') {
      if (!movable) return;
      raiseWidgetWindow(threadId, instanceKey);
      setBounds(windowBounds());
      gesture.current = {
        kind: 'move',
        pointerId: FRAME_DRAG_POINTER,
        startX: drag.screenX,
        startY: drag.screenY,
        last: { x: drag.screenX, y: drag.screenY },
        from: at,
        fromSize: { width: 0, height: 0 },
        moving: true,
      };
    } else if (g?.pointerId === FRAME_DRAG_POINTER) {
      if (drag.phase === 'move') track(g, drag.screenX, drag.screenY);
      else finish(g, drag.screenX, drag.screenY);
    }
  };
  // Read through a ref, so the listener added once always sees this render.
  const frameDrag = useRef(onFrameDrag);
  frameDrag.current = onFrameDrag;
  useEffect(() => {
    const el = ref.current;
    if (!el) return undefined;
    const onDrag = (e: Event) => frameDrag.current((e as CustomEvent<WidgetDrag>).detail);
    el.addEventListener(APP_FRAME_DRAG_EVENT, onDrag);
    return () => el.removeEventListener(APP_FRAME_DRAG_EVENT, onDrag);
  }, []);

  const name = entry.label ?? entry.name;
  const menu = menuTarget(threadId, entry);
  const close = () => setClosedAt(currentRaise(placed) ?? placed.raise);
  const modeMenu = <ModeMenu mode={mode} onPick={(m) => setWidgetWindowMode(threadId, instanceKey, m)} />;
  const closeButton = (
    <button type="button" class="icon-btn header-icon" aria-label={`Close ${name}`} data-tooltip="Close" onClick={close}>
      <CloseIcon />
    </button>
  );

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
    dragAt && !resizeTo ? 'is-dragging' : '',
    // The frame holds the pointer of a drag it started, so it stays live.
    dragAt && gesture.current?.pointerId === FRAME_DRAG_POINTER ? 'is-frame-dragging' : '',
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
      onPointerCancel={cancelGesture}
    >
      {/* One tree in every mode, so a mode change never moves the frame in the
          DOM, which would reload it. */}
      <div class="widget-card widget-card-window">
        {mode === 'window' && (
          <WidgetBar target={menu} icon={entry.icon} compact leading={modeMenu} trailing={closeButton} />
        )}
        <WidgetFrame
          appId={entry.app_id}
          params={entry.params}
          reveal={entry.reveal}
          place="window"
          mode={mode}
          draggable={movable}
          onSize={onSize}
        />
      </div>
      {mode !== 'window' && (
        <div
          class={controlsBelow(mode, at.y) ? 'widget-window-controls is-below' : 'widget-window-controls'}
          role="toolbar"
          aria-label={`${name} window controls`}
        >
          {modeMenu}
          <OverflowMenu ariaLabel={`${name} widget actions`} items={(ctx) => widgetMenuItems(ctx, menu)} />
          {closeButton}
        </div>
      )}
      {mode === 'window' && RESIZE_CORNERS.map((c) => (
        <span key={c} class="widget-window-resize" data-corner={c} aria-hidden="true" />
      ))}
    </div>
  );
}
