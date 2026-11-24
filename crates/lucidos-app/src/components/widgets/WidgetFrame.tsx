import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import { WIDGET_CHANNEL, type ContentSize, type WidgetPlace, type WidgetPush, type WidgetSize } from '@lucidos/widget-frame';
import { AppFrame } from '../apps/AppFrame';
import { appUrl } from '../../api/client';
import { postToAppFrame } from '../../store/actions/app-bridge';
import {
  APP_FRAME_FIT_EVENT, APP_FRAME_MENU_EVENT, APP_FRAME_SIZE_EVENT, type WidgetFit,
} from '../../store/actions/widget-frame-bridge';
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

export const DEFAULT_WIDGET_HEIGHT_PX = 240;

export const OPTION_WIDGET_HEIGHT_PX = 48;

export const OFF_SCREEN_HOLD_MS = 120_000;

export type { WidgetPlace };

export type WidgetFramePlacement =
  | { place: 'inline' | 'dashboard'; size: WidgetSize }
  | { place: Exclude<WidgetPlace, 'inline' | 'dashboard'>; size?: undefined };

export interface WidgetFitProbe {
  measure: string;
  device: string | null;
  onFit: (fit: WidgetFit) => void;
}

export function defaultWidgetHeight(place: WidgetPlace): number {
  return place === 'option' ? OPTION_WIDGET_HEIGHT_PX : DEFAULT_WIDGET_HEIGHT_PX;
}

const lastSizes = new Map<string, ContentSize>();

export function _resetWidgetHeightsForTesting(): void {
  lastSizes.clear();
}

function transcriptBox(host: HTMLElement): HTMLElement {
  return host.closest<HTMLElement>('.widget-box') ?? host;
}

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
    if (Math.abs(shift) >= 1) holdAcrossRelayout(scroller, scroller.scrollTop + shift);
  };
}

function watchInViewport(el: HTMLElement, onChange: (inView: boolean) => void): () => void {
  if (typeof IntersectionObserver !== 'function') {
    onChange(true);
    return () => {};
  }
  const observer = new IntersectionObserver((entries) => {
    const latest = entries[entries.length - 1];
    if (latest) onChange(latest.isIntersecting);
  });
  observer.observe(el);
  return () => observer.disconnect();
}

export function WidgetFrame({
  appId, params, reveal, place, size, visible = true, pans = false, probe, onHeight, onSize, onFrameState, onMenu,
}: WidgetFramePlacement & {
  appId: string;
  params?: WidgetParams;
  reveal: AppReveal;
  visible?: boolean;
  pans?: boolean;
  probe?: WidgetFitProbe;
  onHeight?: (px: number) => void;
  onSize?: (size: ContentSize, fromReport: boolean) => void;
  onFrameState?: (state: FrameState) => void;
  onMenu?: (at?: ViewportPoint) => void;
}) {
  const [host, setHost] = useState<HTMLDivElement | null>(null);
  const frame = useRef<HTMLIFrameElement | null>(null);
  const boxed = size !== undefined;
  const [held, setHeld] = useState(false);
  const probing = probe !== undefined;
  const loaded = held || probing;
  const instanceKey = widgetInstanceKey(appId, params);
  const [height, setHeight] = useState(() => lastSizes.get(instanceKey)?.height ?? defaultWidgetHeight(place));
  const pendingHold = useRef<(() => void) | null>(null);

  const [observed, setObserved] = useState<{ visible: boolean; inView: boolean } | null>(null);
  useEffect(() => {
    if (!host) return undefined;
    const answer = (inView: boolean) => setObserved({ visible, inView });
    return place === 'dashboard' ? watchInViewport(host, answer) : watchOnScreen(transcriptBox(host), answer);
  }, [host, place, visible]);
  const inView = observed?.visible === visible ? observed.inView : null;
  const showing = inView === true && visible;
  useEffect(() => {
    if (showing) {
      setHeld(true);
      return undefined;
    }
    const release = setTimeout(() => setHeld(false), OFF_SCREEN_HOLD_MS);
    return () => clearTimeout(release);
  }, [showing]);

  const guard = useRef(initialHeightGuard(height));
  const onSizeRef = useRef(onSize);
  onSizeRef.current = onSize;
  const onMenuRef = useRef(onMenu);
  onMenuRef.current = onMenu;
  const hasMenu = onMenu !== undefined;
  const probeRef = useRef(probe);
  probeRef.current = probe;
  const measure = probe?.measure;
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
    if (!host) return undefined;
    const answer = (e: Event) => {
      const fit = (e as CustomEvent<WidgetFit>).detail;
      if (fit.token === probeRef.current?.measure) probeRef.current.onFit(fit);
    };
    host.addEventListener(APP_FRAME_FIT_EVENT, answer);
    return () => host.removeEventListener(APP_FRAME_FIT_EVENT, answer);
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
      if (boxed) {
        if (!probeRef.current) lastSizes.set(instanceKey, reported);
        onSizeRef.current?.(reported, true);
        return;
      }
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
      pendingHold.current = holdWhatFollows(transcriptBox(host));
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
  }, [host, appId, instanceKey, place, boxed]);

  useLayoutEffect(() => {
    pendingHold.current?.();
    pendingHold.current = null;
  }, [height]);

  useLayoutEffect(() => onHeight?.(height), [height, onHeight]);

  const initialSize = useRef(lastSizes.get(instanceKey) ?? { height });
  useLayoutEffect(() => onSize?.(initialSize.current, false), []);

  const mark = (el: HTMLIFrameElement) => {
    const push: WidgetPush = { place, visible };
    if (size) push.size = size;
    if (hasMenu) push.menu = true;
    if (pans) push.pans = true;
    if (measure) push.measure = measure;
    return postToAppFrame(el, WIDGET_CHANNEL, push);
  };
  useEffect(() => {
    if (frame.current) mark(frame.current);
  }, [place, size, visible, hasMenu, pans, measure]);

  useEffect(() => {
    if (!held || !host || probing) return undefined;
    return claimWidgetFrame(host, () => setHeld(false));
  }, [held, host, probing]);
  const refreshKey = probing ? 0 : widgetRefreshKey(appId);
  useEffect(() => {
    setRevealed(false);
    if (loaded) guard.current = initialHeightGuard(height);
  }, [loaded, refreshKey]);
  const url = appUrl(appId, undefined, undefined, params, probe ? probe.device : undefined);
  const src = refreshKey > 0 ? cacheBust(url, refreshKey) : url;
  return (
    <div ref={setHost} class={`widget-frame widget-frame-${place}`} style={boxed ? undefined : { height: `${height}px` }}>
      {loaded && (
        <AppFrame
          key={refreshKey}
          src={src}
          reveal={reveal}
          label={{ app: appId, place }}
          onRevealed={() => setRevealed(true)}
          onLoad={(el) => {
            frame.current = el;
            mark(el);
          }}
        />
      )}
    </div>
  );
}
