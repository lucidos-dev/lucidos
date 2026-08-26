import { useEffect, useRef } from 'preact/hooks';
import {
  UI_SCALE_MIN,
  UI_SCALE_MAX,
  UI_SCALE_STEP,
} from '../../store/actions/preferences';
import { Overlay } from './Overlay';
import {
  fractionForScale, scaleAfterKey, scaleAtPointer, type TrackMetrics,
} from './scaleSlider';
import {
  scaleModalOpen,
  previewScale,
  closeScaleModal,
  adjustUiScale,
  previewSliderValue,
  commitSliderValue,
} from './scaleModalState';

export function ScaleModal() {
  const isOpen = scaleModalOpen.value;
  const rowRef = useRef<HTMLDivElement>(null);
  const thumbRef = useRef<HTMLDivElement>(null);
  /** The pointer that owns the drag, and the row it was measured against. */
  const drag = useRef<{ pointerId: number; track: TrackMetrics } | null>(null);

  useEffect(() => {
    function handleWheel(e: WheelEvent) {
      if (!(e.metaKey || e.ctrlKey)) return;
      e.preventDefault();
      adjustUiScale(e.deltaY < 0 ? UI_SCALE_STEP : -UI_SCALE_STEP);
    }
    document.addEventListener('wheel', handleWheel, { passive: false });
    return () => document.removeEventListener('wheel', handleWheel);
  }, []);

  if (!isOpen) return null;

  /** Measured once per gesture: the modal cannot resize under a finger, and
   *  reading the box on every move would force a layout after each thumb move. */
  function measureRow(): TrackMetrics | null {
    const row = rowRef.current;
    if (!row) return null;
    const rect = row.getBoundingClientRect();
    return {
      left: rect.left,
      width: rect.width,
      thumbWidth: thumbRef.current?.getBoundingClientRect().width ?? 0,
    };
  }

  function handlePointerDown(e: PointerEvent) {
    if (!e.isPrimary) return;
    const track = measureRow();
    if (!track) return;
    const row = e.currentTarget as HTMLElement;
    // Own the whole gesture. Without capture a finger that slides off the row,
    // or past either end of the track, stops delivering moves.
    row.setPointerCapture(e.pointerId);
    drag.current = { pointerId: e.pointerId, track };
    e.preventDefault();
    // preventDefault suppresses the focus the press would otherwise bring, and
    // the arrow keys below need it, so take it explicitly.
    row.focus({ preventScroll: true });
    const next = scaleAtPointer(e.clientX, track);
    if (next !== null) previewSliderValue(next);
  }

  function handlePointerMove(e: PointerEvent) {
    const active = drag.current;
    if (active?.pointerId !== e.pointerId) return;
    e.preventDefault();
    const next = scaleAtPointer(e.clientX, active.track);
    if (next !== null) previewSliderValue(next);
  }

  /** The drag this event ends, or `null` when it owns none. */
  function endDrag(e: PointerEvent): { track: TrackMetrics } | null {
    const active = drag.current;
    if (active?.pointerId !== e.pointerId) return null;
    drag.current = null;
    return active;
  }

  /** A released drag commits where the finger left off. */
  function handlePointerUp(e: PointerEvent) {
    const active = endDrag(e);
    if (!active) return;
    commitSliderValue(scaleAtPointer(e.clientX, active.track) ?? previewScale.peek());
  }

  /** A cancelled drag commits what is on screen instead of remapping the event.
   *  `pointercancel` carries a last known position in name only, and a browser
   *  that reports zeroes would map to the far left and persist 75% silently. */
  function handlePointerCancel(e: PointerEvent) {
    if (!endDrag(e)) return;
    commitSliderValue(previewScale.peek());
  }

  function handleKeyDown(e: KeyboardEvent) {
    const next = scaleAfterKey(e.key, previewScale.peek());
    if (next === null) return;
    e.preventDefault();
    e.stopPropagation();
    commitSliderValue(next);
  }

  return (
    <Overlay open onClose={closeScaleModal} overlayClass="scale-modal-overlay" panelClass="scale-modal">
      <div class="scale-modal-label">{previewScale.value}%</div>
      <div
        ref={rowRef}
        class="scale-modal-slider"
        role="slider"
        aria-label="UI scale"
        aria-valuemin={UI_SCALE_MIN}
        aria-valuemax={UI_SCALE_MAX}
        aria-valuenow={previewScale.value}
        aria-valuetext={`${previewScale.value}%`}
        tabIndex={0}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
        onPointerCancel={handlePointerCancel}
        onKeyDown={handleKeyDown}
      >
        <div class="scale-modal-track" />
        <div
          ref={thumbRef}
          class="scale-modal-thumb"
          style={{ '--scale-slider-fraction': fractionForScale(previewScale.value) }}
        />
      </div>
      <div class="scale-modal-range">
        <span>{UI_SCALE_MIN}%</span>
        <span>{UI_SCALE_MAX}%</span>
      </div>
    </Overlay>
  );
}
