import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import { WIDGET_CHANNEL, type ContentSize, type WidgetPlace, type WidgetPush, type WidgetSize } from '@lucidos/widget-frame';
import { AppFrame } from '../apps/AppFrame';
import { appUrl } from '../../api/client';
import { postToAppFrame } from '../../store/actions/app-bridge';
import { APP_FRAME_SIZE_EVENT } from '../../store/actions/widget-frame-bridge';
import { watchOnScreen } from '../../hooks/useOnScreenInTranscript';
import { holdAcrossRelayout } from '../chat/scrollState';
import { widgetRefreshKey } from '../../store/widgets';
import type { AppReveal } from '../../store/types';
import { cacheBust } from '../apps/AppFrame';
import { widgetInstanceKey, type WidgetParams } from '../../utils/widgetParams';
import { claimWidgetFrame } from './widgetFrameBudget';

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

/** Watch whether `el` crosses the viewport. A Home board tile scrolls in its
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
 *  so it mounts at once, and it caps its height. A `home` frame fills its Home
 *  board tile, whose size class sets the box: a report never sizes it
 *  (ADR 0432). It mounts once on screen, and a hidden board holds it as an
 *  off-screen transcript frame is held. Every place shares one cap on mounted
 *  frames (`widgetFrameBudget.ts`).
 *
 *  `params` are the instance's *widget params*, carried in the frame URL.
 *  `size` is a Home board tile's widget size, and `visible` false while a
 *  loaded frame is out of sight. Both and the place are pushed to the frame on
 *  load and on each change.
 *
 *  `onHeight` hears the frame's height on mount and on each report, and
 *  `onSize` the whole size, saying whether the widget reported it. */
export function WidgetFrame({ appId, params, reveal, place, size, visible = true, onHeight, onSize }: {
  appId: string;
  params?: WidgetParams;
  reveal: AppReveal;
  place: WidgetPlace;
  size?: WidgetSize;
  visible?: boolean;
  onHeight?: (px: number) => void;
  onSize?: (size: ContentSize, fromReport: boolean) => void;
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
  const [inView, setInView] = useState(eager);
  useEffect(() => {
    if (eager || !host) return undefined;
    return place === 'home' ? watchInViewport(host, setInView) : watchOnScreen(transcriptBox(host), setInView);
  }, [host, eager, place]);
  const showing = inView && visible;
  useEffect(() => {
    if (eager) return undefined;
    if (showing) {
      setHeld(true);
      return undefined;
    }
    const release = setTimeout(() => setHeld(false), OFF_SCREEN_HOLD_MS);
    return () => clearTimeout(release);
  }, [showing, eager]);

  useEffect(() => {
    if (!host) return;
    const onReport = (e: Event) => {
      const reported = (e as CustomEvent<ContentSize>).detail;
      lastSizes.set(instanceKey, reported);
      // Taken while the old height is laid out. A shelf or board frame
      // is outside the transcript, so its height moves nothing in it.
      pendingHold.current = eager || place === 'home' ? null : holdWhatFollows(transcriptBox(host));
      setHeight(reported.height);
      onSize?.(reported, true);
    };
    host.addEventListener(APP_FRAME_SIZE_EVENT, onReport);
    return () => host.removeEventListener(APP_FRAME_SIZE_EVENT, onReport);
  }, [host, instanceKey, eager, place, onSize]);

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
    return postToAppFrame(el, WIDGET_CHANNEL, push);
  };
  useEffect(() => {
    if (frame.current) mark(frame.current);
  }, [place, size, visible]);

  // Evicted by the shared cap: drawn again when it next comes on screen.
  useEffect(() => {
    if (!held || !host) return undefined;
    return claimWidgetFrame(host, () => setHeld(false));
  }, [held, host]);
  // A change to the widget's files bumps this, which remounts the frame on a
  // fresh URL, as the Canvas pane's refresh key does.
  const refreshKey = widgetRefreshKey(appId);
  const url = appUrl(appId, undefined, undefined, params);
  const src = refreshKey > 0 ? cacheBust(url, refreshKey) : url;
  return (
    <div ref={setHost} class={`widget-frame widget-frame-${place}`} style={place === 'home' ? undefined : { height: `${height}px` }}>
      {held && (
        <AppFrame
          key={refreshKey}
          src={src}
          reveal={reveal}
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
