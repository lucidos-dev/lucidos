import type { ComponentChildren, JSX } from 'preact';
import { useCallback, useLayoutEffect, useRef, useState } from 'preact/hooks';
import type { ViewportPoint } from '../../hooks/useAnchoredPopover';
import type { AppReveal, WidgetSize } from '../../store/types';
import type { WidgetParams } from '../../utils/widgetParams';
import { WidgetFrame, type WidgetPlace } from './WidgetFrame';
import type { FrameState } from './widgetFrameState';

type BoxAttributes = Omit<JSX.HTMLAttributes<HTMLDivElement>, 'ref' | 'class' | 'size'>;

/** A *widget box*: the fixed box a widget draws into at one widget size, in a
 *  thread card and as a dashboard tile alike (ADR 0432). The size sets the box
 *  and the frame fills it. A report never sizes it, and it never scrolls
 *  inside: content past it is clipped under a fade.
 *
 *  `boxRef` hands the box element to its owner, and `hideFade` keeps the fade
 *  off while something else covers the box, such as edit mode's shield. The
 *  rest of the attributes land on the box, so a tile's grid place and handlers
 *  sit on the box itself. */
export function WidgetBox({
  appId, params, reveal, place, size, transparent = false, visible, onFrameState, onMenu,
  class: className, boxRef, hideFade = false, children, ...attributes
}: BoxAttributes & {
  appId: string;
  params?: WidgetParams;
  reveal: AppReveal;
  place: Extract<WidgetPlace, 'inline' | 'dashboard'>;
  size: WidgetSize;
  transparent?: boolean;
  visible?: boolean;
  onFrameState?: (state: FrameState) => void;
  onMenu?: (at?: ViewportPoint) => void;
  class?: string;
  boxRef?: (el: HTMLDivElement | null) => void;
  hideFade?: boolean;
  children?: ComponentChildren;
}) {
  const [box, setBox] = useState<HTMLDivElement | null>(null);
  const [contentHeight, setContentHeight] = useState(0);
  const [clipped, setClipped] = useState(false);
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
    const measure = () => setClipped(contentHeight - box.clientHeight > 1);
    measure();
    if (typeof ResizeObserver !== 'function') return undefined;
    const observer = new ResizeObserver(measure);
    observer.observe(box);
    return () => observer.disconnect();
  }, [box, contentHeight]);

  const classes = ['widget-box', className, transparent && 'is-transparent'].filter(Boolean).join(' ');
  return (
    <div
      {...attributes}
      ref={attach}
      class={classes}
      data-size={size}
    >
      <WidgetFrame
        appId={appId}
        params={params}
        reveal={reveal}
        place={place}
        size={size}
        visible={visible}
        onSize={(s) => setContentHeight(s.height)}
        onFrameState={onFrameState}
        onMenu={onMenu}
      />
      {clipped && !hideFade && <div class="widget-box-fade" />}
      {children}
    </div>
  );
}
