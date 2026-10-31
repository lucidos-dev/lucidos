import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import { WIDGET_CHANNEL, type ContentSize, type WidgetPlace, type WidgetPush, type WidgetSize } from '@lucidos/widget-frame';
import { AppFrame } from '../apps/AppFrame';
import { appUrl } from '../../api/client';
import { postToAppFrame } from '../../store/actions/app-bridge';
import { APP_FRAME_MENU_EVENT, APP_FRAME_SIZE_EVENT } from '../../store/actions/widget-frame-bridge';
import type { ViewportPoint } from '../../hooks/useAnchoredPopover';
import { watchOnScreen } from '../../hooks/useOnScreenInTranscript';
import { holdAcrossRelayout } from '../chat/scrollState';
import { widgetRefreshKey } from '../../store/widgets';
import type { AppReveal } from '../../store/types';
import { cacheBust } from '../apps/AppFrame';
import { widgetInstanceKey, type WidgetParams } from '../../utils/widgetParams';
import { claimWidgetFrame } from './widgetFrameBudget';
import { decideHeight, initialHeightGuard } from './widgetHeightGuard';
import { postClientLog } from '../../utils/clientLog';
import { widgetFrameState, type FrameState } from './widgetFrameState';

/** The frame's height before the widget reports one, in CSS pixels. */
export const DEFAULT_WIDGET_HEIGHT_PX = 240;

/** An option's widget starts one control row tall: an option holds a small
 *  widget, and a card of four options must not jump by a screen as each one
 *  reports. */
export const OPTION_WIDGET_HEIGHT_PX = 48;

/** How long a frame stays loaded after it scrolls off screen. Scrolling back
 *  inside it finds the widget as it was, a playing clip included. */
export const OFF_SCREEN_HOLD_MS = 120_000;

export type { WidgetPlace };

/** A frame's height before its widget reports one. */
export function defaultWidgetHeight(place: WidgetPlace): number {
  return place === 'option' ? OPTION_WIDGET_HEIGHT_PX : DEFAULT_WIDGET_HEIGHT_PX;
}

/** The last size each widget instance reported. An instance opened again
 *  starts at its own size. An inline frame that unmounts off screen keeps it as its
 *  placeholder, so the transcript does not jump. */
const lastSizes = new Map<string, ContentSize>();

/** Test-only: forget every remembered size. */
export function _resetWidgetHeightsForTesting(): void {
  lastSizes.clear();
}

/** The box a frame takes up in the transcript: the card's clip, which caps a
 *  tall inline frame, else the frame itself. */
function transcriptBox(host: HTMLElement): HTMLElement {
  return host.closest<HTMLElement>('.inline-widget-clip') ?? host;
}

/** Hold what follows `box` still while it changes height across or above the
 *  reader's line, the transcript's top edge. A frame mounts as it comes on
 *  screen and then reports, so a reader scrolling up meets it at that line.
 *  The transcript turns off browser scroll anchoring, so nothing else holds it.
 *
 *  A frame the reader is using keeps its place instead, as anything pressed
 *  does (ADR 0147), but only while it is on screen. Scrolling never blurs a
 *  frame, and one held off screen still reports. A box with nothing after it on
 *  screen keeps its place too.
 *
 *  Returns the correction to run once the new height is laid out, or null when
 *  the change cannot move what the reader is reading. */
function holdWhatFollows(box: HTMLElement): (() => void) | null {
  const scroller = box.closest<HTMLElement>('.thread-content');
  if (!scroller) return null;
  const band = scroller.getBoundingClientRect();
  const before = box.getBoundingClientRect();
  if (before.top >= band.top || before.bottom >= band.bottom) return null;
  if (before.bottom > band.top && box.contains(document.activeElement)) return null;
  const bottomBefore = before.bottom - band.top;
  return () => {
    const shift = box.getBoundingClientRect().bottom - scroller.getBoundingClientRect().top - bottomBefore;
    // A write stops an iOS fling, so a sub-pixel shift is left alone.
    if (Math.abs(shift) >= 1) holdAcrossRelayout(scroller, scroller.scrollTop + shift);
  };
}

/** Watch whether `el` crosses the viewport. A dashboard tile scrolls in its
 *  own pane, never the transcript, so the transcript's band cannot answer. */
function watchInViewport(el: HTMLElement, onChange: (inView: boolean) => void): () => void {
  if (typeof IntersectionObserver !== 'function') {
    onChange(true);
    return () => {};
  }
  // The observer's own answer counts every clipping ancestor and a pane slid
  // sideways off screen, which a rect against the window does not.
  const observer = new IntersectionObserver((entries) => {
    const latest = entries[entries.length - 1];
    if (latest) onChange(latest.isIntersecting);
  });
  observer.observe(el);
  return () => observer.disconnect();
}

/** A widget's app frame, sized to what it draws (ADRs 0402, 0407).
 *
 *  `place` decides two things. An `inline` frame sits in an *inline widget*
 *  in the transcript. It takes the full reported height and never scrolls, and
 *  the inline widget clips it. It mounts once it first comes on screen, since
 *  each frame is a renderer process of its own. Scrolled off, it stays for
 *  `OFF_SCREEN_HOLD_MS` and then unloads. An `option` frame sits in a question
 *  option and mounts the same way.
 *
 *  A `shelf` frame drops open under the title. It is on screen by definition,
 *  so it mounts at once, and it caps its height. A `dashboard` frame fills
 *  its dashboard tile, whose size class sets the box: a report never sizes it
 *  (ADR 0432). It mounts once on screen, and a hidden dashboard holds it as an
 *  off-screen transcript frame is held. Every place shares one cap on mounted
 *  frames (`widgetFrameBudget.ts`).
 *
 *  `params` are the instance's *widget params*, carried in the frame URL.
 *  `size` is a dashboard tile's widget size, and `visible` false while a
 *  loaded frame is out of sight. Both and the place are pushed to the frame on
 *  load and on each change.
 *
 *  A report sizes the frame only through the widget height guard
 *  (`widgetHeightGuard.ts`), at most once per animation frame.
 *  `onHeight` hears the frame's height on mount and on each applied report,
 *  and `onSize` the whole size, saying whether the widget reported it.
 *
 *  `onFrameState` hears where the frame stands (`widgetFrameState.ts`) on mount
 *  and on each change. A dashboard waits on it to show once its frames have
 *  loaded.
 *
 *  `onMenu` opens the widget's menu. Given one, the frame is marked `menu`, so
 *  an unclaimed right-click inside it lands there, at its point in the
 *  viewport. A frame with no menu leaves its right-click alone. */
export function WidgetFrame({ appId, params, reveal, place, size, visible = true, onHeight, onSize, onFrameState, onMenu }: {
  appId: string;
  params?: WidgetParams;
  reveal: AppReveal;
  place: WidgetPlace;
  size?: WidgetSize;
  visible?: boolean;
  onHeight?: (px: number) => void;
  onSize?: (size: ContentSize, fromReport: boolean) => void;
  onFrameState?: (state: FrameState) => void;
  onMenu?: (at?: ViewportPoint) => void;
}) {
  const [host, setHost] = useState<HTMLDivElement | null>(null);
  const frame = useRef<HTMLIFrameElement | null>(null);
  const eager = place === 'shelf';
  // Set on screen, cleared by the off-screen hold or by eviction. A shelf
  // frame starts held and is never set again.
  const [held, setHeld] = useState(eager);
  const instanceKey = widgetInstanceKey(appId, params);
  const [height, setHeight] = useState(() => lastSizes.get(instanceKey)?.height ?? defaultWidgetHeight(place));
  // Only `onReport` changes the height, and it sets this first.
  const pendingHold = useRef<(() => void) | null>(null);

  // The card clips a tall frame, so the box to watch is the clip it draws in.
  // An answer belongs to the visibility it was given under: a frame shown again
  // watches afresh, and until the new answer lands its place is unknown.
  const [observed, setObserved] = useState<{ visible: boolean; inView: boolean } | null>(null);
  useEffect(() => {
    if (eager || !host) return undefined;
    const answer = (inView: boolean) => setObserved({ visible, inView });
    return place === 'dashboard' ? watchInViewport(host, answer) : watchOnScreen(transcriptBox(host), answer);
  }, [host, eager, place, visible]);
  const inView = eager ? true : observed?.visible === visible ? observed.inView : null;
  const showing = inView === true && visible;
  useEffect(() => {
    if (eager) return undefined;
    if (showing) {
      setHeld(true);
      return undefined;
    }
    const release = setTimeout(() => setHeld(false), OFF_SCREEN_HOLD_MS);
    return () => clearTimeout(release);
  }, [showing, eager]);

  // Lives for the mount, so a remount follows its widget afresh.
  const guard = useRef(initialHeightGuard(height));
  const onSizeRef = useRef(onSize);
  onSizeRef.current = onSize;
  const onMenuRef = useRef(onMenu);
  onMenuRef.current = onMenu;
  const hasMenu = onMenu !== undefined;
  const [revealed, setRevealed] = useState(false);
  const frameState = widgetFrameState(inView, showing, revealed);
  const onFrameStateRef = useRef(onFrameState);
  onFrameStateRef.current = onFrameState;
  useLayoutEffect(() => onFrameStateRef.current?.(frameState), [frameState]);

  useEffect(() => {
    if (!host) return undefined;
    const open = (e: Event) => onMenuRef.current?.((e as CustomEvent<ViewportPoint | null>).detail ?? undefined);
    host.addEventListener(APP_FRAME_MENU_EVENT, open);
    return () => host.removeEventListener(APP_FRAME_MENU_EVENT, open);
  }, [host]);

  useEffect(() => {
    if (!host) return;
    let pending: ContentSize | null = null;
    let frameRequest = 0;
    const apply = (now: number) => {
      frameRequest = 0;
      if (!pending) return;
      const reported = pending;
      pending = null;
      const { state, decision } = decideHeight(guard.current, reported.height, now);
      guard.current = state;
      if (decision.kind === 'ignore') return;
      if (decision.kind === 'lock') {
        postClientLog('widget', 'height guard locked a frame', {
          app: appId, place, reason: decision.reason, reported: reported.height, held: decision.height,
        });
      }
      const size = { ...reported, height: decision.height };
      lastSizes.set(instanceKey, size);
      // Taken while the old height is laid out. A shelf or dashboard frame
      // is outside the transcript, so its height moves nothing in it.
      pendingHold.current = eager || place === 'dashboard' ? null : holdWhatFollows(transcriptBox(host));
      setHeight(size.height);
      onSizeRef.current?.(size, true);
    };
    const onReport = (e: Event) => {
      pending = (e as CustomEvent<ContentSize>).detail;
      if (!frameRequest) frameRequest = requestAnimationFrame(apply);
    };
    host.addEventListener(APP_FRAME_SIZE_EVENT, onReport);
    return () => {
      host.removeEventListener(APP_FRAME_SIZE_EVENT, onReport);
      cancelAnimationFrame(frameRequest);
    };
  }, [host, appId, instanceKey, eager, place]);

  useLayoutEffect(() => {
    pendingHold.current?.();
    pendingHold.current = null;
  }, [height]);

  useLayoutEffect(() => onHeight?.(height), [height, onHeight]);

  // The remembered size, so a frame opens at it before the first report.
  // Only on mount: each later report reaches `onSize` itself.
  const initialSize = useRef(lastSizes.get(instanceKey) ?? { height });
  useLayoutEffect(() => onSize?.(initialSize.current, false), []);

  const mark = (el: HTMLIFrameElement) => {
    const push: WidgetPush = { place, visible };
    if (size) push.size = size;
    if (hasMenu) push.menu = true;
    return postToAppFrame(el, WIDGET_CHANNEL, push);
  };
  useEffect(() => {
    if (frame.current) mark(frame.current);
  }, [place, size, visible, hasMenu]);

  // Evicted by the shared cap: drawn again when it next comes on screen.
  useEffect(() => {
    if (!held || !host) return undefined;
    return claimWidgetFrame(host, () => setHeld(false));
  }, [held, host]);
  // A change to the widget's files bumps this, which remounts the frame on a
  // fresh URL, as the Canvas pane's refresh key does.
  const refreshKey = widgetRefreshKey(appId);
  // Each fresh document gets a fresh guard: a reload or a redraw after
  // eviction may be a fixed widget, so an old lock must not hold it. It is
  // covered anew too.
  useEffect(() => {
    setRevealed(false);
    if (held) guard.current = initialHeightGuard(height);
  }, [held, refreshKey]);
  const url = appUrl(appId, undefined, undefined, params);
  const src = refreshKey > 0 ? cacheBust(url, refreshKey) : url;
  return (
    <div ref={setHost} class={`widget-frame widget-frame-${place}`} style={place === 'dashboard' ? undefined : { height: `${height}px` }}>
      {held && (
        <AppFrame
          key={refreshKey}
          src={src}
          reveal={reveal}
          label={{ app: appId, place }}
          onRevealed={() => setRevealed(true)}
          // The mark that makes the SDK report the content size.
          onLoad={(el) => {
            frame.current = el;
            mark(el);
          }}
        />
      )}
    </div>
  );
}
