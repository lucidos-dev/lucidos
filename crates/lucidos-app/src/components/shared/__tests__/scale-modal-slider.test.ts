import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { preferences } from '../../../store/store';
import { setPreference } from '../../../api/client';
import { UI_SCALE_MAX, UI_SCALE_STEP } from '../../../store/actions/preferences';
import {
  scaleModalOpen,
  previewScale,
  previewSliderValue,
  commitSliderValue,
  closeScaleModal,
  dismissScaleModal,
  openScaleModal,
  adjustUiScale,
  SHORTCUT_LINGER_MS,
  _resetScaleTimersForTesting,
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
    _resetScaleTimersForTesting();
    setPreferenceMock.mockClear();
    vi.useFakeTimers();
    preferences.value = { status: 'loaded', data: { 'ui-scale': '100' } };
    scaleModalOpen.value = true;
    previewScale.value = 100;
  });

  afterEach(() => {
    _resetScaleTimersForTesting();
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
    _resetScaleTimersForTesting();
    setPreferenceMock.mockClear();
    vi.useFakeTimers();
    preferences.value = { status: 'loaded', data: { 'ui-scale': '100' } };
    scaleModalOpen.value = true;
    previewScale.value = 100;
  });

  afterEach(() => {
    _resetScaleTimersForTesting();
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

  it('a Cmd held past the debounce does not spend a second identical write', async () => {
    adjustUiScale(UI_SCALE_STEP);
    vi.advanceTimersByTime(500);
    await settle();
    expect(setPreferenceMock).toHaveBeenCalledTimes(1);

    dismissScaleModal();
    await settle();
    expect(setPreferenceMock).toHaveBeenCalledTimes(1);
  });
});

/**
 * Regression: the panel's only way out was a `keyup` on Meta/Control, and that
 * event is not guaranteed to arrive. The packaged macOS app is the reported
 * case: zoom with the shortcut there and the panel stayed on screen for the
 * rest of the session. It now counts itself out after `SHORTCUT_LINGER_MS` of
 * no change, with the release kept as the fast path.
 *
 * The countdown belongs to the shortcut, not to the panel. Settings opens the
 * same component as a real modal, and one that dissolved while the user reached
 * for its slider would be a second bug.
 */
describe('scale panel linger dismiss', () => {
  const settle = () => Promise.resolve().then(() => {}).then(() => {});

  beforeEach(() => {
    _resetScaleTimersForTesting();
    setPreferenceMock.mockClear();
    vi.useFakeTimers();
    preferences.value = { status: 'loaded', data: { 'ui-scale': '100' } };
    // Closed, so a zoom step exercises the open-and-arm path the shortcut takes.
    scaleModalOpen.value = false;
    previewScale.value = 100;
  });

  afterEach(() => {
    _resetScaleTimersForTesting();
    vi.useRealTimers();
  });

  it('a zoom step opens the panel and closes it again once the user stops', async () => {
    adjustUiScale(UI_SCALE_STEP);
    expect(scaleModalOpen.value).toBe(true);

    vi.advanceTimersByTime(SHORTCUT_LINGER_MS - 100);
    expect(scaleModalOpen.value).toBe(true);

    vi.advanceTimersByTime(200);
    await settle();
    expect(scaleModalOpen.value).toBe(false);
    // The debounce already delivered it, so lingering out adds no second write.
    expect(setPreferenceMock).toHaveBeenCalledTimes(1);
    expect(setPreferenceMock).toHaveBeenCalledWith('ui-scale', '112.5', expect.any(String));
  });

  it('each further step pushes the deadline out', () => {
    adjustUiScale(UI_SCALE_STEP);
    vi.advanceTimersByTime(SHORTCUT_LINGER_MS - 100);
    adjustUiScale(UI_SCALE_STEP);
    vi.advanceTimersByTime(SHORTCUT_LINGER_MS - 100);
    expect(scaleModalOpen.value).toBe(true);
    expect(previewScale.value).toBe(125);

    vi.advanceTimersByTime(200);
    expect(scaleModalOpen.value).toBe(false);
  });

  it('a step that is already at the clamp keeps the panel up', () => {
    // Walk to the ceiling, then keep pressing: the value stops moving but the
    // user has not stopped asking.
    for (let i = 0; i < 10; i++) adjustUiScale(UI_SCALE_STEP);
    expect(previewScale.value).toBe(UI_SCALE_MAX);

    vi.advanceTimersByTime(SHORTCUT_LINGER_MS - 100);
    adjustUiScale(UI_SCALE_STEP);
    vi.advanceTimersByTime(200);
    expect(scaleModalOpen.value).toBe(true);
  });

  // Renewing on each `input` was the first shape, and it dissolved the panel
  // under a thumb held still mid-drag: renewing measures idleness, and a
  // stationary thumb is idle. Reaching for the slider settles it instead.
  it('grabbing the slider settles the panel, so a held thumb cannot dissolve it', () => {
    adjustUiScale(UI_SCALE_STEP);
    previewSliderValue(150);
    vi.advanceTimersByTime(SHORTCUT_LINGER_MS * 4);
    expect(scaleModalOpen.value).toBe(true);
    expect(previewScale.value).toBe(150);
  });

  it('a tap straight onto the track settles it too, with no preview first', () => {
    adjustUiScale(UI_SCALE_STEP);
    commitSliderValue(150);
    vi.advanceTimersByTime(SHORTCUT_LINGER_MS * 4);
    expect(scaleModalOpen.value).toBe(true);
  });

  it('Escape disarms the countdown, so the cancelled value is never re-persisted', async () => {
    adjustUiScale(UI_SCALE_STEP);
    closeScaleModal();
    expect(scaleModalOpen.value).toBe(false);

    vi.advanceTimersByTime(SHORTCUT_LINGER_MS * 2);
    await settle();
    expect(setPreferenceMock).not.toHaveBeenCalled();
    expect(scaleModalOpen.value).toBe(false);
  });

  it('the Settings modal never counts itself out', () => {
    openScaleModal();
    vi.advanceTimersByTime(SHORTCUT_LINGER_MS * 4);
    expect(scaleModalOpen.value).toBe(true);
  });

  it('a zoom step inside the Settings modal does not start a countdown', () => {
    openScaleModal();
    adjustUiScale(UI_SCALE_STEP);
    previewSliderValue(150);
    vi.advanceTimersByTime(SHORTCUT_LINGER_MS * 4);
    expect(scaleModalOpen.value).toBe(true);
  });
});
