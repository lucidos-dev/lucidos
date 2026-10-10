import { useEffect, useLayoutEffect, useState } from 'preact/hooks';
import { WIDGET_CHANNEL } from '@lucidos/widget-height';
import { AppFrame } from '../apps/AppFrame';
import { appUrl } from '../../api/client';
import { postToAppFrame } from '../../store/actions/app-bridge';
import { APP_FRAME_HEIGHT_EVENT } from '../../store/actions/app-height-bridge';
import { watchOnScreen } from '../../hooks/useOnScreenInTranscript';
import { widgetRefreshKey } from '../../store/widgets';
import type { AppReveal } from '../../store/types';
import { cacheBust } from '../apps/AppFrame';

/** The frame's height before the widget reports one, in CSS pixels. */
export const DEFAULT_WIDGET_HEIGHT_PX = 240;

/** The last height each widget reported. A widget opened again starts at its
 *  own size. An inline frame that unmounts off screen keeps it as its
 *  placeholder, so the transcript does not jump. */
const lastHeights = new Map<string, number>();

/** Test-only: forget every remembered height. */
export function _resetWidgetHeightsForTesting(): void {
  lastHeights.clear();
}

/** A widget's app frame, sized to what it draws (ADRs 0402, 0407).
 *
 *  `place` decides two things. An `inline` frame sits in the transcript's
 *  widget card. It takes the full reported height and never scrolls, and the
 *  card clips it. It mounts only while on screen, since each frame is a
 *  renderer process of its own. A `shelf` frame drops open under the title. It
 *  is on screen by definition, so it mounts at once, and it caps its height.
 *
 *  `onHeight` hears the frame's height on mount and on each report. */
export function WidgetFrame({ appId, reveal, place, onHeight }: {
  appId: string;
  reveal: AppReveal;
  place: 'inline' | 'shelf';
  onHeight?: (px: number) => void;
}) {
  const [host, setHost] = useState<HTMLDivElement | null>(null);
  const [onScreen, setOnScreen] = useState(false);
  const [height, setHeight] = useState(() => lastHeights.get(appId) ?? DEFAULT_WIDGET_HEIGHT_PX);
  const eager = place === 'shelf';

  // The card clips a tall frame, so the box to watch is the clip it draws in.
  useEffect(() => {
    if (eager || !host) return undefined;
    return watchOnScreen(host.closest<HTMLElement>('.widget-card-clip') ?? host, setOnScreen);
  }, [host, eager]);

  useEffect(() => {
    if (!host) return;
    const onReport = (e: Event) => {
      const reported = (e as CustomEvent<number>).detail;
      lastHeights.set(appId, reported);
      setHeight(reported);
    };
    host.addEventListener(APP_FRAME_HEIGHT_EVENT, onReport);
    return () => host.removeEventListener(APP_FRAME_HEIGHT_EVENT, onReport);
  }, [host, appId]);

  useLayoutEffect(() => onHeight?.(height), [height, onHeight]);

  const mounted = eager || onScreen;
  // A change to the widget's files bumps this, which remounts the frame on a
  // fresh URL, as the Canvas pane's refresh key does.
  const refreshKey = widgetRefreshKey(appId);
  const src = refreshKey > 0 ? cacheBust(appUrl(appId), refreshKey) : appUrl(appId);
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
