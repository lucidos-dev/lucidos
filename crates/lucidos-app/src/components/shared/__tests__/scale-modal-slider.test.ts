import { describe, it, expect, beforeEach, vi } from 'vitest';
import { preferences } from '../../../store/store';
import {
  scaleModalOpen,
  previewScale,
  previewSliderValue,
  commitSliderValue,
  closeScaleModal,
} from '../scaleModalState';

// setPreference would otherwise POST to the API; just confirm the call goes
// out, swallow the network round trip.
vi.mock('../../../api/client', () => ({
  getPreferences: vi.fn(),
  setPreference: vi.fn().mockResolvedValue(undefined),
}));

/**
 * Regression: ScaleModal's onChange handler used to call
 * `scaleModalOpen.value = false` after persisting. On macOS Chrome (and every
 * other browser) the `change` event on `<input type="range">` fires on
 * mouse-up after the drag commits — so the modal popped closed the instant
 * the user finished any drag, before they could read the picked value.
 *
 * The modal must now only close via Escape, backdrop click, or
 * dismissScaleModal() (Cmd/Ctrl release). The slider commit just persists.
 */
describe('scale modal slider drag', () => {
  beforeEach(() => {
    preferences.value = { status: 'loaded', data: { 'ui-scale': '100' } };
    scaleModalOpen.value = true;
    previewScale.value = 100;
  });

  it('previewSliderValue updates the preview without closing the modal', () => {
    previewSliderValue(130);
    expect(previewScale.value).toBe(125);
    expect(scaleModalOpen.value).toBe(true);
  });

  it('commitSliderValue persists the value and leaves the modal open', () => {
    commitSliderValue(130);
    expect(previewScale.value).toBe(125);
    expect(scaleModalOpen.value).toBe(true);
  });

  it('successive drag/release cycles never close the modal', () => {
    previewSliderValue(110);
    commitSliderValue(110);
    expect(scaleModalOpen.value).toBe(true);

    previewSliderValue(150);
    commitSliderValue(150);
    expect(scaleModalOpen.value).toBe(true);
    expect(previewScale.value).toBe(150);
  });

  it('clamps out-of-range values to UI_SCALE_MIN/MAX', () => {
    previewSliderValue(10);
    expect(previewScale.value).toBe(75);

    commitSliderValue(9999);
    expect(previewScale.value).toBe(200);
  });

  it('closeScaleModal still dismisses explicitly (sanity for Escape/backdrop)', () => {
    closeScaleModal();
    expect(scaleModalOpen.value).toBe(false);
  });
});
