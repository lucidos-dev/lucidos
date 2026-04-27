import { signal } from '@preact/signals';
import { useEffect } from 'preact/hooks';
import { applyUiScale, setUiScale, currentUiScale, UI_SCALE_MIN, UI_SCALE_MAX, UI_SCALE_STEP, UI_SCALE_DEFAULT } from '../../store/actions/preferences';
import { ModalOverlay } from './ModalOverlay';

export const scaleModalOpen = signal(false);
const previewScale = signal(100);

let saveTimeout: ReturnType<typeof setTimeout> | undefined;

export function openScaleModal() {
  previewScale.value = currentUiScale();
  scaleModalOpen.value = true;
}

export function closeScaleModal() {
  clearTimeout(saveTimeout);
  scaleModalOpen.value = false;
  const saved = currentUiScale();
  if (previewScale.value !== saved) applyUiScale(saved);
}

export function dismissScaleModal() {
  clearTimeout(saveTimeout);
  setUiScale(previewScale.value);
  scaleModalOpen.value = false;
}

function applyScaleChange(next: number) {
  const current = scaleModalOpen.value ? previewScale.value : currentUiScale();
  if (next === current) return;
  previewScale.value = next;
  applyUiScale(next);
  if (!scaleModalOpen.value) scaleModalOpen.value = true;
  clearTimeout(saveTimeout);
  saveTimeout = setTimeout(() => setUiScale(next), 500);
}

export function resetUiScale() {
  applyScaleChange(UI_SCALE_DEFAULT);
}

export function adjustUiScale(delta: number) {
  const base = scaleModalOpen.value ? previewScale.value : currentUiScale();
  applyScaleChange(Math.max(UI_SCALE_MIN, Math.min(UI_SCALE_MAX, base + delta)));
}

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
    if (!isNaN(val) && val !== previewScale.value) {
      previewScale.value = val;
      applyUiScale(val);
    }
  }

  function handleSliderChange(e: Event) {
    const val = parseInt((e.target as HTMLInputElement).value, 10);
    if (!isNaN(val)) {
      setUiScale(val);
      scaleModalOpen.value = false;
    }
  }

  return (
    <ModalOverlay onClose={closeScaleModal} class="scale-modal-overlay">
      <div class="scale-modal">
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
      </div>
    </ModalOverlay>
  );
}
