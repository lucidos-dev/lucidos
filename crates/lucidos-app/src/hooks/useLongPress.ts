import { hapticTick } from '@lucidos/haptic';
import { LONG_PRESS_DELAY_MS, PRESS_MOVE_TOLERANCE_PX } from '@lucidos/widget-frame';
import { useEffect, useRef } from 'preact/hooks';
import type { ViewportPoint } from './useAnchoredPopover';

export interface LongPressHandlers {
  onPointerDown(e: PointerEvent): void;
  onPointerMove(e: PointerEvent): void;
  onPointerUp(e: PointerEvent): void;
  onPointerLeave(e: PointerEvent): void;
  onPointerCancel(e: PointerEvent): void;
  onContextMenu(e: MouseEvent): void;
  onClick(e: MouseEvent): void;
  cancel(): void;
}

export interface LongPressOptions {
  delayMs?: number;
  moveTolerancePx?: number;
}

export { LONG_PRESS_DELAY_MS };
const DEFAULT_MOVE_TOLERANCE_PX = PRESS_MOVE_TOLERANCE_PX;
const SUPPRESS_FUSE_MS = 700;
const NATIVE_LONG_PRESS_PROPERTIES = ['user-select', '-webkit-user-select', '-webkit-touch-callout'];

function holdNativeLongPress(target: HTMLElement): () => void {
  const { style } = target;
  const before = NATIVE_LONG_PRESS_PROPERTIES.map((property) =>
    [property, style.getPropertyValue(property), style.getPropertyPriority(property)] as const);
  for (const property of NATIVE_LONG_PRESS_PROPERTIES) style.setProperty(property, 'none', 'important');
  const release = () => {
    document.removeEventListener('pointerdown', release, true);
    for (const [property, value, priority] of before) {
      if (value) style.setProperty(property, value, priority);
      else style.removeProperty(property);
    }
  };
  document.addEventListener('pointerdown', release, true);
  return release;
}

export type LongPressCallback = (target: HTMLElement, at?: ViewportPoint) => void;

export function makeLongPressHandlers(
  onLongPress: LongPressCallback,
  onClick: (e: MouseEvent) => void,
  opts: LongPressOptions = {},
): LongPressHandlers {
  const delayMs = opts.delayMs ?? LONG_PRESS_DELAY_MS;
  const moveTolerance = opts.moveTolerancePx ?? DEFAULT_MOVE_TOLERANCE_PX;

  let timer: ReturnType<typeof setTimeout> | null = null;
  let fuse: ReturnType<typeof setTimeout> | null = null;
  let startX = 0;
  let startY = 0;
  let armed = false;
  let unwatchLift: (() => void) | null = null;
  let releaseNativeLongPress: (() => void) | null = null;

  function endPress() {
    if (timer !== null) { clearTimeout(timer); timer = null; }
    unwatchLift?.();
    unwatchLift = null;
  }
  function fire(target: HTMLElement) {
    endPress();
    armed = true;
    hapticTick();
    onLongPress(target);
  }
  function watchLift(pointerId: number) {
    const samePointer = (e: PointerEvent) => { if (e.pointerId === pointerId) endPress(); };
    const lastFinger = (e: TouchEvent) => { if (e.touches.length === 0) endPress(); };
    const nextPress = () => endPress();
    document.addEventListener('pointerdown', nextPress, true);
    document.addEventListener('pointerup', samePointer, true);
    document.addEventListener('pointercancel', samePointer, true);
    document.addEventListener('touchend', lastFinger, true);
    document.addEventListener('touchcancel', lastFinger, true);
    unwatchLift = () => {
      document.removeEventListener('pointerdown', nextPress, true);
      document.removeEventListener('pointerup', samePointer, true);
      document.removeEventListener('pointercancel', samePointer, true);
      document.removeEventListener('touchend', lastFinger, true);
      document.removeEventListener('touchcancel', lastFinger, true);
    };
  }
  function disarm() {
    armed = false;
    if (fuse !== null) { clearTimeout(fuse); fuse = null; }
  }
  function armFuse() {
    if (fuse !== null) clearTimeout(fuse);
    fuse = setTimeout(() => { fuse = null; armed = false; }, SUPPRESS_FUSE_MS);
  }

  return {
    onPointerDown(e) {
      if (e.button !== 0) return;
      disarm();
      endPress();
      startX = e.clientX;
      startY = e.clientY;
      const target = e.currentTarget as HTMLElement;
      timer = setTimeout(() => fire(target), delayMs);
      if (e.pointerType !== 'mouse') releaseNativeLongPress = holdNativeLongPress(target);
      watchLift(e.pointerId);
    },
    onPointerMove(e) {
      if (timer === null) return;
      if (Math.abs(e.clientX - startX) > moveTolerance ||
          Math.abs(e.clientY - startY) > moveTolerance) {
        endPress();
      }
    },
    onPointerUp() {
      endPress();
      if (armed && fuse === null) armFuse();
    },
    onPointerLeave() {
      endPress();
    },
    onPointerCancel() {
      endPress();
      disarm();
    },
    onContextMenu(e) {
      e.preventDefault();
      endPress();
      armed = true;
      armFuse();
      onLongPress(e.currentTarget as HTMLElement, { x: e.clientX, y: e.clientY });
    },
    onClick(e) {
      if (armed) {
        disarm();
        e.preventDefault();
        e.stopPropagation();
        return;
      }
      onClick(e);
    },
    cancel() {
      endPress();
      disarm();
      releaseNativeLongPress?.();
      releaseNativeLongPress = null;
    },
  };
}

export function useLongPress(
  onLongPress: LongPressCallback,
  onClick: (e: MouseEvent) => void,
  opts?: LongPressOptions,
): LongPressHandlers {
  const longPressRef = useRef(onLongPress);
  longPressRef.current = onLongPress;
  const clickRef = useRef(onClick);
  clickRef.current = onClick;
  const handlersRef = useRef<LongPressHandlers>();
  if (!handlersRef.current) {
    handlersRef.current = makeLongPressHandlers(
      (t, at) => longPressRef.current(t, at),
      (e) => clickRef.current(e),
      opts,
    );
  }
  useEffect(() => () => handlersRef.current?.cancel(), []);
  return handlersRef.current;
}
