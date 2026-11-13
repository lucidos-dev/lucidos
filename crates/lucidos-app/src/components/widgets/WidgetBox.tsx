import type { ComponentChildren, JSX } from 'preact';
import { useCallback, useLayoutEffect, useRef, useState } from 'preact/hooks';
import type { ViewportPoint } from '../../hooks/useAnchoredPopover';
import type { ContentSize } from '@lucidos/widget-frame';
import type { AppReveal, WidgetSize } from '../../store/types';
import { widgetBoxCss } from '../../utils/dashboardGeometry';
import type { WidgetParams } from '../../utils/widgetParams';
import { WidgetFrame, type WidgetFitProbe, type WidgetPlace } from './WidgetFrame';
import type { FrameState } from './widgetFrameState';

/** How long a resize cover waits for the widget's report before it clears
 *  anyway: a frame not loaded, or held off screen, never reports. */
export const RESIZE_COVER_FUSE_MS = 1000;

type BoxAttributes = Omit<JSX.HTMLAttributes<HTMLDivElement>, 'ref' | 'class' | 'size'>;

/** A *widget box*: the fixed box a widget draws into at one widget size, in a
 *  thread card and as a dashboard tile alike (ADR 0432). The size sets the box
 *  and the frame fills it. A report never sizes it, and it never scrolls
 *  inside: content past it is clipped under a fade.
 *
 *  A size change covers the box at once, before the frame hears the new size.
 *  The frame is its own renderer process and relays out a frame or two later.
 *  The cover fades once the widget reports a layout for the new size,
 *  or when `RESIZE_COVER_FUSE_MS` runs out. So the widget never draws its old
 *  layout in the new box.
 *
 *  The frame sits in a sideways scroller. A box the host clamps below its true
 *  width, which only an XL box can be, scrolls sideways, and the widget still
 *  sees `xl`. The frame hears that it pans, so a sideways drag scrolls the box.
 *  Content wider than that canvas is clipped under a right-edge fade. A
 *  transparent box draws no fade on either edge.
 *
 *  `probe` makes the frame a *widget fit check*'s (`WidgetFitProbe`).
 *
 *  `boxRef` hands the box element to its owner, and `hideFade` keeps the fade
 *  off while something else covers the box, such as edit mode's shield. The
 *  rest of the attributes land on the box, so a tile's grid place and handlers
 *  sit on the box itself. */
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
  // The size the box waits on the widget to lay out for, else null.
  const [coveredFor, setCoveredFor] = useState<WidgetSize | null>(null);
  const shownSize = useRef(size);
  const boxRefNow = useRef(boxRef);
  boxRefNow.current = boxRef;
  // Stable, so a render never hands the box a null and back.
  const attach = useCallback((el: HTMLDivElement | null) => {
    setBox(el);
    boxRefNow.current?.(el);
  }, []);

  useLayoutEffect(() => {
    if (!box) return undefined;
    // One pixel of slack absorbs `clientHeight` rounding a fractional report.
    const measure = () => setClipped(content.height - box.clientHeight > 1);
    measure();
    if (typeof ResizeObserver !== 'function') return undefined;
    const observer = new ResizeObserver(measure);
    observer.observe(box);
    return () => observer.disconnect();
  }, [box, content.height]);

  useLayoutEffect(() => {
    if (!canvas) return undefined;
    // Against the canvas, not the box: an XL box that pans is not cut.
    const measure = () => setCutRight(content.width !== undefined && content.width - canvas.clientWidth > 1);
    measure();
    if (typeof ResizeObserver !== 'function') return undefined;
    const observer = new ResizeObserver(measure);
    observer.observe(canvas);
    return () => observer.disconnect();
  }, [canvas, content.width]);

  // A layout effect, so the cover is up in the same paint as the new box.
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
    // The same pixel of slack, for a true width that rounds past the room.
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
