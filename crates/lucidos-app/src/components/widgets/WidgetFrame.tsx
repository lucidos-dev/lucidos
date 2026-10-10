import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import { WIDGET_CHANNEL } from '@lucidos/widget-height';
import { AppFrame } from '../apps/AppFrame';
import { appUrl } from '../../api/client';
import { postToAppFrame } from '../../store/actions/app-bridge';
import { APP_FRAME_HEIGHT_EVENT } from '../../store/actions/app-height-bridge';
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

/** Where a widget frame sits (ADRs 0407, 0415). */
export type WidgetPlace = 'inline' | 'shelf' | 'option' | 'embed';

/** A frame's height before its widget reports one. */
export function defaultWidgetHeight(place: WidgetPlace): number {
  return place === 'option' ? OPTION_WIDGET_HEIGHT_PX : DEFAULT_WIDGET_HEIGHT_PX;
}

/** The last height each widget instance reported. An instance opened again
 *  starts at its own size. An inline frame that unmounts off screen keeps it as its
 *  placeholder, so the transcript does not jump. */
const lastHeights = new Map<string, number>();

/** Test-only: forget every remembered height. */
export function _resetWidgetHeightsForTesting(): void {
  lastHeights.clear();
}

/** The box a frame takes up in the transcript: the card's clip, which caps a
 *  tall inline frame, else the frame itself. */
function transcriptBox(host: HTMLElement): HTMLElement {
  return host.closest<HTMLElement>('.widget-card-clip') ?? host;
}

/** Hold what follows `box` still while it changes height across or above the
 *  reader's line, the transcript's top edge. A frame mounts as it comes on
 *  screen and then reports, so a reader scrolling up meets it at that line.
 *  The transcript turns off browser scroll anchoring, so nothing else holds it.
 *
 *  A frame the reader is using keeps its place instead, as anything pressed
 *  does (ADR 0147). So does a box with nothing after it on screen.
 *
 *  Returns the correction to run once the new height is laid out, or null when
 *  the change cannot move what the reader is reading. */
function holdWhatFollows(box: HTMLElement): (() => void) | null {
  const scroller = box.closest<HTMLElement>('.thread-content');
  if (!scroller || box.contains(document.activeElement)) return null;
  const band = scroller.getBoundingClientRect();
  const before = box.getBoundingClientRect();
  if (before.top >= band.top || before.bottom >= band.bottom) return null;
  const bottomBefore = before.bottom - band.top;
  return () => {
    const shift = box.getBoundingClientRect().bottom - scroller.getBoundingClientRect().top - bottomBefore;
    // A write stops an iOS fling, so a sub-pixel shift is left alone.
    if (Math.abs(shift) >= 1) holdAcrossRelayout(scroller, scroller.scrollTop + shift);
  };
}

/** A widget's app frame, sized to what it draws (ADRs 0402, 0407).
 *
 *  `place` decides two things. An `inline` frame sits in the transcript's
 *  widget card. It takes the full reported height and never scrolls, and the
 *  card clips it. It mounts only while on screen, since each frame is a
 *  renderer process of its own. An `option` frame sits in a question option
 *  and an `embed` frame in a reply; both mount the same way. A `shelf` frame
 *  drops open under the title. It is on screen by definition, so it mounts at
 *  once, and it caps its height. Every place shares one cap on mounted frames
 *  (`widgetFrameBudget.ts`).
 *
 *  `params` are the instance's *widget params*, carried in the frame URL.
 *
 *  `onHeight` hears the frame's height on mount and on each report. */
export function WidgetFrame({ appId, params, reveal, place, onHeight }: {
  appId: string;
  params?: WidgetParams;
  reveal: AppReveal;
  place: WidgetPlace;
  onHeight?: (px: number) => void;
}) {
  const [host, setHost] = useState<HTMLDivElement | null>(null);
  const [onScreen, setOnScreen] = useState(false);
  const instanceKey = widgetInstanceKey(appId, params);
  const [height, setHeight] = useState(() => lastHeights.get(instanceKey) ?? defaultWidgetHeight(place));
  const eager = place === 'shelf';
  // Only `onReport` changes the height, and it sets this first.
  const pendingHold = useRef<(() => void) | null>(null);

  // The card clips a tall frame, so the box to watch is the clip it draws in.
  useEffect(() => {
    if (eager || !host) return undefined;
    return watchOnScreen(transcriptBox(host), setOnScreen);
  }, [host, eager]);

  useEffect(() => {
    if (!host) return;
    const onReport = (e: Event) => {
      const reported = (e as CustomEvent<number>).detail;
      lastHeights.set(instanceKey, reported);
      // Taken while the old height is laid out. A shelf frame floats over the
      // transcript, so its height moves nothing in it.
      pendingHold.current = eager ? null : holdWhatFollows(transcriptBox(host));
      setHeight(reported);
    };
    host.addEventListener(APP_FRAME_HEIGHT_EVENT, onReport);
    return () => host.removeEventListener(APP_FRAME_HEIGHT_EVENT, onReport);
  }, [host, instanceKey, eager]);

  useLayoutEffect(() => {
    pendingHold.current?.();
    pendingHold.current = null;
  }, [height]);

  useLayoutEffect(() => onHeight?.(height), [height, onHeight]);

  // Evicted by the shared cap: drawn again when it next comes on screen.
  const [evicted, setEvicted] = useState(false);
  const wanted = eager || onScreen;
  useEffect(() => {
    setEvicted(false);
    if (!wanted || !host) return undefined;
    return claimWidgetFrame(host, () => setEvicted(true));
  }, [wanted, host]);

  const mounted = wanted && !evicted;
  // A change to the widget's files bumps this, which remounts the frame on a
  // fresh URL, as the Canvas pane's refresh key does.
  const refreshKey = widgetRefreshKey(appId);
  const url = appUrl(appId, undefined, undefined, params);
  const src = refreshKey > 0 ? cacheBust(url, refreshKey) : url;
  return (
    <div ref={setHost} class={`widget-frame widget-frame-${place}`} style={{ height: `${height}px` }}>
      {mounted && (
        <AppFrame
          key={refreshKey}
          src={src}
          reveal={reveal}
          // The mark that makes the SDK report the content height.
          onLoad={(frame) => postToAppFrame(frame, WIDGET_CHANNEL, {})}
        />
      )}
    </div>
  );
}
