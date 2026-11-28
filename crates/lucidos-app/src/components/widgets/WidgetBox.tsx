import type { ComponentChildren, JSX } from 'preact';
import { useCallback, useLayoutEffect, useRef, useState } from 'preact/hooks';
import type { ViewportPoint } from '../../hooks/useAnchoredPopover';
import type { ContentSize } from '@lucidos/widget-frame';
import type { AppReveal, WidgetSize } from '../../store/types';
import { widgetBoxCss } from '../../utils/dashboardGeometry';
import type { WidgetParams } from '../../utils/widgetParams';
import { WidgetFrame, type WidgetFitProbe, type WidgetPlace } from './WidgetFrame';
import type { FrameState } from './widgetFrameState';

export const RESIZE_COVER_FUSE_MS = 1000;

type BoxAttributes = Omit<JSX.HTMLAttributes<HTMLDivElement>, 'ref' | 'class' | 'size'>;

export function WidgetBox({
  appId, params, reveal, place, size, transparent = false, visible, probe, onFrameState, onMenu,
  class: className, boxRef, hideFade = false, children, ...attributes
}: BoxAttributes & {
  appId: string;
  params?: WidgetParams;
  reveal: AppReveal;
  place: Extract<WidgetPlace, 'inline' | 'dashboard'>;
  size: WidgetSize;
  transparent?: boolean;
  visible?: boolean;
  probe?: WidgetFitProbe;
  onFrameState?: (state: FrameState) => void;
  onMenu?: (at?: ViewportPoint) => void;
  class?: string;
  boxRef?: (el: HTMLDivElement | null) => void;
  hideFade?: boolean;
  children?: ComponentChildren;
}) {
  const [box, setBox] = useState<HTMLDivElement | null>(null);
  const [scroller, setScroller] = useState<HTMLDivElement | null>(null);
  const [pans, setPans] = useState(false);
  const [canvas, setCanvas] = useState<HTMLDivElement | null>(null);
  const [content, setContent] = useState<ContentSize>({ height: 0 });
  const [clipped, setClipped] = useState(false);
  const [cutRight, setCutRight] = useState(false);
  const [coveredFor, setCoveredFor] = useState<WidgetSize | null>(null);
  const shownSize = useRef(size);
  const boxRefNow = useRef(boxRef);
  boxRefNow.current = boxRef;
  const attach = useCallback((el: HTMLDivElement | null) => {
    setBox(el);
    boxRefNow.current?.(el);
  }, []);

  useLayoutEffect(() => {
    if (!box) return undefined;
    const measure = () => setClipped(content.height - box.clientHeight > 1);
    measure();
    if (typeof ResizeObserver !== 'function') return undefined;
    const observer = new ResizeObserver(measure);
    observer.observe(box);
    return () => observer.disconnect();
  }, [box, content.height]);

  useLayoutEffect(() => {
    if (!canvas) return undefined;
    const measure = () => setCutRight(content.width !== undefined && content.width - canvas.clientWidth > 1);
    measure();
    if (typeof ResizeObserver !== 'function') return undefined;
    const observer = new ResizeObserver(measure);
    observer.observe(canvas);
    return () => observer.disconnect();
  }, [canvas, content.width]);

  useLayoutEffect(() => {
    if (shownSize.current === size) return undefined;
    shownSize.current = size;
    setCoveredFor(size);
    const fuse = setTimeout(() => setCoveredFor(null), RESIZE_COVER_FUSE_MS);
    return () => clearTimeout(fuse);
  }, [size]);

  const onSize = (s: ContentSize) => {
    setContent(s);
    if (s.size !== undefined) setCoveredFor((waiting) => (waiting === s.size ? null : waiting));
  };

  useLayoutEffect(() => {
    if (!scroller) return undefined;
    const measure = () => setPans(scroller.scrollWidth - scroller.clientWidth > 1);
    measure();
    if (typeof ResizeObserver !== 'function') return undefined;
    const observer = new ResizeObserver(measure);
    observer.observe(scroller);
    return () => observer.disconnect();
  }, [scroller, size]);

  const classes = ['widget-box', className, transparent && 'is-transparent'].filter(Boolean).join(' ');
  return (
    <div
      {...attributes}
      ref={attach}
      class={classes}
      data-size={size}
    >
      <div ref={setScroller} class="widget-box-scroll">
        <div ref={setCanvas} class="widget-box-canvas" style={{ '--widget-box-true-width': widgetBoxCss(size).width }}>
          <WidgetFrame
            appId={appId}
            params={params}
            reveal={reveal}
            place={place}
            size={size}
            visible={visible}
            pans={pans}
            probe={probe}
            onSize={onSize}
            onFrameState={onFrameState}
            onMenu={onMenu}
          />
        </div>
      </div>
      {!transparent && !hideFade && clipped && <div class="widget-box-fade" data-edge="bottom" />}
      {!transparent && !hideFade && cutRight && <div class="widget-box-fade" data-edge="right" />}
      <div class={`widget-box-cover${coveredFor ? ' is-covering' : ''}`} aria-hidden="true" />
      {children}
    </div>
  );
}
