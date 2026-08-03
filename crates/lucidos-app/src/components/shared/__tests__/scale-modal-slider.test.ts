import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { preferences } from '../../../store/store';
import { setPreference } from '../../../api/client';
import { UI_SCALE_STEP } from '../../../store/actions/preferences';
import {
  scaleModalOpen,
  previewScale,
  previewSliderValue,
  commitSliderValue,
  closeScaleModal,
  dismissScaleModal,
  adjustUiScale,
  _resetScheduledSaveForTesting,
} from '../scaleModalState';

// setPreference would otherwise POST to the API; just confirm the call goes
// out, swallow the network round trip. `isTransientFetchError` is real logic
// savePreference classifies rejections with, so give it the honest shape rather
// than a stub that would misroute a failure.
vi.mock('../../../api/client', () => ({
  getPreferences: vi.fn(),
  setPreference: vi.fn().mockResolvedValue(undefined),
  isTransientFetchError: (err: unknown) => err instanceof DOMException
    && (err.name === 'AbortError' || err.name === 'TimeoutError'),
}));

const setPreferenceMock = vi.mocked(setPreference);

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
    _resetScheduledSaveForTesting();
    setPreferenceMock.mockClear();
    vi.useFakeTimers();
    preferences.value = { status: 'loaded', data: { 'ui-scale': '100' } };
    scaleModalOpen.value = true;
    previewScale.value = 100;
  });

  afterEach(() => {
    _resetScheduledSaveForTesting();
    vi.useRealTimers();
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

/**
 * Regression: `commitSliderValue` used to fire an immediate, un-debounced
 * `PUT /preferences` per `change` event, with no coalescing of in-flight
 * requests. WebKit is generous with `change` during a range drag, so on an iOS
 * PWA one drag queued several concurrent writes, and one page suspend then
 * failed all of them at once. That is the three stacked "Failed to save
 * ui-scale preference: request cancelled" cards this suite now pins shut.
 */
describe('scale write coalescing', () => {
  // Deliveries are serialized on a per-key promise chain (see
  // `store/actions/preferences.ts`), so the request goes out a microtask after
  // the debounce fires rather than inside `advanceTimersByTime`. Drain them, or
  // every assertion below reads the mock before the write was ever attempted.
  const settle = () => Promise.resolve().then(() => {}).then(() => {});

  beforeEach(() => {
    _resetScheduledSaveForTesting();
    setPreferenceMock.mockClear();
    vi.useFakeTimers();
    preferences.value = { status: 'loaded', data: { 'ui-scale': '100' } };
    scaleModalOpen.value = true;
    previewScale.value = 100;
  });

  afterEach(() => {
    _resetScheduledSaveForTesting();
    vi.useRealTimers();
  });

  it('one drag is one write, carrying the value the user released on', async () => {
    commitSliderValue(112.5);
    commitSliderValue(125);
    commitSliderValue(137.5);
    await settle();
    expect(setPreferenceMock).not.toHaveBeenCalled();

    vi.advanceTimersByTime(500);
    await settle();
    expect(setPreferenceMock).toHaveBeenCalledTimes(1);
    expect(setPreferenceMock).toHaveBeenCalledWith('ui-scale', '137.5', expect.any(String));
  });

  // `applyUiScale` is the local half: the CSS var plus the localStorage cache
  // the FOUC script reads. It must stay instant, so the debounce delays only
  // the network write. (The test env stubs `documentElement.style` to a no-op,
  // so localStorage is the observable half here.)
  it('applies each commit locally at once, without waiting out the debounce', async () => {
    commitSliderValue(125);
    await settle();
    expect(localStorage.getItem('lucidos-ui-scale')).toBe('125');
    expect(setPreferenceMock).not.toHaveBeenCalled();
  });

  it('a preview arriving mid-debounce re-points the armed save at what is on screen', async () => {
    commitSliderValue(125);
    previewSliderValue(150);
    vi.advanceTimersByTime(500);
    await settle();
    expect(setPreferenceMock).toHaveBeenCalledTimes(1);
    expect(setPreferenceMock).toHaveBeenCalledWith('ui-scale', '150', expect.any(String));
  });

  it('closing after a commit persists it, because a debounced drag must not silently revert', async () => {
    commitSliderValue(125);
    closeScaleModal();
    await settle();
    expect(scaleModalOpen.value).toBe(false);
    expect(setPreferenceMock).toHaveBeenCalledTimes(1);
    expect(setPreferenceMock).toHaveBeenCalledWith('ui-scale', '125', expect.any(String));
    // The flush disarmed the timer; nothing fires a second write later.
    vi.advanceTimersByTime(1000);
    await settle();
    expect(setPreferenceMock).toHaveBeenCalledTimes(1);
  });

  it('closing after preview only still reverts to the stored scale', async () => {
    previewSliderValue(150);
    closeScaleModal();
    vi.advanceTimersByTime(1000);
    await settle();
    expect(setPreferenceMock).not.toHaveBeenCalled();
    expect(localStorage.getItem('lucidos-ui-scale')).toBe('100');
  });

  it('closing after a keyboard zoom step still cancels it (Escape is the cancel)', async () => {
    adjustUiScale(UI_SCALE_STEP);
    closeScaleModal();
    vi.advanceTimersByTime(1000);
    await settle();
    expect(setPreferenceMock).not.toHaveBeenCalled();
  });

  it('dismissScaleModal (Cmd release) saves immediately and disarms the debounce', async () => {
    adjustUiScale(UI_SCALE_STEP);
    dismissScaleModal();
    await settle();
    expect(setPreferenceMock).toHaveBeenCalledTimes(1);
    expect(setPreferenceMock).toHaveBeenCalledWith('ui-scale', '112.5', expect.any(String));
    vi.advanceTimersByTime(1000);
    await settle();
    expect(setPreferenceMock).toHaveBeenCalledTimes(1);
  });
});
