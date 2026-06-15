import { useEffect } from 'preact/hooks';
import {
  UI_SCALE_MIN,
  UI_SCALE_MAX,
  UI_SCALE_STEP,
} from '../../store/actions/preferences';
import { Overlay } from './Overlay';
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

  function handleSliderInput(e: Event) {
    const val = parseInt((e.target as HTMLInputElement).value, 10);
    if (!isNaN(val)) previewSliderValue(val);
  }

  function handleSliderChange(e: Event) {
    const val = parseInt((e.target as HTMLInputElement).value, 10);
    if (!isNaN(val)) commitSliderValue(val);
  }

  return (
    <Overlay open onClose={closeScaleModal} overlayClass="scale-modal-overlay" panelClass="scale-modal">
      <div class="scale-modal-label">{previewScale.value}%</div>
      <input
        type="range"
        class="scale-modal-slider"
        min={UI_SCALE_MIN}
        max={UI_SCALE_MAX}
        step={UI_SCALE_STEP}
        value={previewScale.value}
        tabIndex={-1}
        onInput={handleSliderInput}
        onChange={handleSliderChange}
      />
      <div class="scale-modal-range">
        <span>{UI_SCALE_MIN}%</span>
        <span>{UI_SCALE_MAX}%</span>
      </div>
    </Overlay>
  );
}
