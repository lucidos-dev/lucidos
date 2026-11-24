import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { preferences } from '../../../store/store';
import { getPreferences, setPreference } from '../../../api/client';
import {
  UI_SCALE_MAX, UI_SCALE_STEP, loadPreferences, _resetPendingPreferenceWritesForTesting,
} from '../../../store/actions/preferences';
import {
  scaleModalOpen,
  previewScale,
  previewSliderValue,
  commitSliderValue,
  closeScaleModal,
  dismissScaleModal,
  openScaleModal,
  adjustUiScale,
  renewScaleModalLinger,
  SHORTCUT_LINGER_MS,
  _resetScaleTimersForTesting,
} from '../scaleModalState';

vi.mock('../../../api/client', () => ({
  getPreferences: vi.fn(),
  setPreference: vi.fn().mockResolvedValue(undefined),
  retryTransient: (read: () => Promise<unknown>) => read(),
  isTransientFetchError: (err: unknown) => err instanceof DOMException
    && (err.name === 'AbortError' || err.name === 'TimeoutError'),
}));

const setPreferenceMock = vi.mocked(setPreference);

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

describe('scale write coalescing', () => {
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

describe('a preferences refetch keeps the scale on screen', () => {
  const settle = () => Promise.resolve().then(() => {}).then(() => {});
  const getPreferencesMock = vi.mocked(getPreferences);

  beforeEach(() => {
    _resetScaleTimersForTesting();
    _resetPendingPreferenceWritesForTesting();
    setPreferenceMock.mockClear();
    vi.useFakeTimers();
    preferences.value = { status: 'loaded', data: { 'ui-scale': '100' } };
    getPreferencesMock.mockResolvedValue({ preferences: { 'ui-scale': '100' } });
    scaleModalOpen.value = false;
    previewScale.value = 100;
  });

  afterEach(() => {
    _resetScaleTimersForTesting();
    _resetPendingPreferenceWritesForTesting();
    vi.useRealTimers();
  });

  it('while the save waits out its debounce', async () => {
    adjustUiScale(UI_SCALE_STEP);
    await loadPreferences();
    expect(localStorage.getItem('lucidos-ui-scale')).toBe('112.5');
  });

  it('while the save is in flight', async () => {
    let accept: () => void = () => {};
    setPreferenceMock.mockImplementationOnce(() => new Promise((resolve) => {
      accept = () => resolve({ success: true });
    }));
    adjustUiScale(UI_SCALE_STEP);
    vi.advanceTimersByTime(500);
    await settle();
    expect(setPreferenceMock).toHaveBeenCalledTimes(1);

    await loadPreferences();
    expect(localStorage.getItem('lucidos-ui-scale')).toBe('112.5');

    accept();
    await settle();
    await loadPreferences();
    expect(localStorage.getItem('lucidos-ui-scale')).toBe('100');
  });

  it('but a cancelled step lets the stored scale back', async () => {
    adjustUiScale(UI_SCALE_STEP);
    closeScaleModal();
    await loadPreferences();
    expect(localStorage.getItem('lucidos-ui-scale')).toBe('100');
  });
});

describe('scale panel linger dismiss', () => {
  const settle = () => Promise.resolve().then(() => {}).then(() => {});

  beforeEach(() => {
    _resetScaleTimersForTesting();
    setPreferenceMock.mockClear();
    vi.useFakeTimers();
    preferences.value = { status: 'loaded', data: { 'ui-scale': '100' } };
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
    for (let i = 0; i < 10; i++) adjustUiScale(UI_SCALE_STEP);
    expect(previewScale.value).toBe(UI_SCALE_MAX);

    vi.advanceTimersByTime(SHORTCUT_LINGER_MS - 100);
    adjustUiScale(UI_SCALE_STEP);
    vi.advanceTimersByTime(200);
    expect(scaleModalOpen.value).toBe(true);
  });

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

  it('a gesture that has not yet earned a step still keeps the panel up', () => {
    adjustUiScale(UI_SCALE_STEP);
    for (let i = 0; i < 4; i++) {
      vi.advanceTimersByTime(SHORTCUT_LINGER_MS - 100);
      renewScaleModalLinger();
    }
    expect(scaleModalOpen.value).toBe(true);

    vi.advanceTimersByTime(SHORTCUT_LINGER_MS + 100);
    expect(scaleModalOpen.value).toBe(false);
  });

  it('renewing never starts a countdown on the Settings modal', () => {
    openScaleModal();
    renewScaleModalLinger();
    vi.advanceTimersByTime(SHORTCUT_LINGER_MS * 4);
    expect(scaleModalOpen.value).toBe(true);
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
