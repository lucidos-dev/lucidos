
export const MIN_THUMB_PX = 24;

export const MAX_THUMB_FRACTION = 0.25;

export interface ScrollEventContext {
  userScrolling: boolean;
  programmaticScroll: boolean;
  repaintNudge: boolean;
}

export interface IndicatorVisibility {
  shown: boolean;
  armHideTimer: boolean;
}

export function nextIndicatorVisibility(
  shown: boolean,
  ev: ScrollEventContext,
): IndicatorVisibility {
  if (ev.repaintNudge && !ev.userScrolling) return { shown, armHideTimer: false };

  const suppressed = ev.programmaticScroll && !ev.userScrolling;
  const nextShown = shown || ev.userScrolling;
  return { shown: nextShown, armHideTimer: nextShown && !suppressed };
}

export function counterScaledRadiusPx(halfWidthPx: number, scaleY: number): number {
  if (!(scaleY > 0) || !Number.isFinite(scaleY)) return halfWidthPx;
  return halfWidthPx / scaleY;
}

export interface ScrollIndicatorInput {
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
  trackHeightPx: number;
}

export interface ScrollIndicatorGeometry {
  visible: boolean;
  thumbHeightPx: number;
  thumbOffsetPx: number;
}

const HIDDEN: ScrollIndicatorGeometry = { visible: false, thumbHeightPx: 0, thumbOffsetPx: 0 };

export function computeScrollIndicator(input: ScrollIndicatorInput): ScrollIndicatorGeometry {
  const { scrollHeight, clientHeight, trackHeightPx } = input;
  if (!(trackHeightPx > 0) || !(clientHeight > 0) || !(scrollHeight > clientHeight)) return HIDDEN;

  const maxScroll = scrollHeight - clientHeight;
  const scrollTop = Math.min(Math.max(0, input.scrollTop), maxScroll);

  const rawThumb = (clientHeight / scrollHeight) * trackHeightPx;
  const ceiling = Math.min(trackHeightPx, Math.max(MIN_THUMB_PX, trackHeightPx * MAX_THUMB_FRACTION));
  const thumbHeightPx = Math.min(ceiling, Math.max(MIN_THUMB_PX, rawThumb));

  const thumbOffsetPx = (scrollTop / maxScroll) * (trackHeightPx - thumbHeightPx);

  return { visible: true, thumbHeightPx, thumbOffsetPx };
}
