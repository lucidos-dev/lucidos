import { beforeEach, describe, expect, test, vi } from 'vitest';
import type { DeepLinkTarget } from './notification-deeplink';

const isPageActiveMock = vi.fn(() => true);
const isInViewportMock = vi.fn(() => false);
const focusedThreadIdSignal = { value: null as string | null };

vi.mock('../../utils/pageActive', () => ({ isPageActive: isPageActiveMock }));
vi.mock('../../utils/viewport', () => ({ isInViewport: isInViewportMock }));
vi.mock('../store', async () => {
  const actual = await vi.importActual<typeof import('../store')>('../store');
  return { ...actual, focusedThreadId: focusedThreadIdSignal };
});

const importModule = async () => await import('./in-app-notification-toast');

function target(opts: {
  notification?: string | null;
  thread?: string | null;
  event?: string | null;
}): DeepLinkTarget {
  return {
    notification: opts.notification ?? 'n-1',
    thread: opts.thread ?? null,
    event: opts.event ?? null,
    tap: { kind: 'modal' },
  };
}

beforeEach(() => {
  isPageActiveMock.mockReset().mockReturnValue(true);
  isInViewportMock.mockReset().mockReturnValue(false);
  focusedThreadIdSignal.value = null;
});


describe('§4 in-app surface matrix', () => {
  test('s4_row1_focused_event_in_viewport_classifies_as_auto_read', async () => {
    isPageActiveMock.mockReturnValue(true);
    isInViewportMock.mockReturnValue(true);
    focusedThreadIdSignal.value = 't-1';
    const { classifyInAppRow } = await importModule();
    expect(classifyInAppRow(target({ thread: 't-1', event: 'e-1' }))).toBe('row1_auto_read');
  });

  test('s4_row2_focused_scrolled_away_classifies_as_toast', async () => {
    isPageActiveMock.mockReturnValue(true);
    isInViewportMock.mockReturnValue(false);
    focusedThreadIdSignal.value = 't-1';
    const { classifyInAppRow } = await importModule();
    expect(classifyInAppRow(target({ thread: 't-1', event: 'e-1' }))).toBe(
      'row2_or_3_toast_and_badge',
    );
  });

  test('s4_row3_active_other_thread_classifies_as_toast', async () => {
    isPageActiveMock.mockReturnValue(true);
    focusedThreadIdSignal.value = 't-2';
    const { classifyInAppRow } = await importModule();
    expect(classifyInAppRow(target({ thread: 't-1', event: 'e-1' }))).toBe(
      'row2_or_3_toast_and_badge',
    );
  });

  test('s4_row4_hidden_classifies_as_shell_not_active_regardless_of_focus_or_viewport', async () => {
    isPageActiveMock.mockReturnValue(false);
    isInViewportMock.mockReturnValue(true);
    focusedThreadIdSignal.value = 't-1';
    const { classifyInAppRow } = await importModule();
    expect(classifyInAppRow(target({ thread: 't-1', event: 'e-1' }))).toBe('shell_not_active');
  });

  test('s4_null_event_id_with_focused_thread_falls_through_to_toast', async () => {
    isPageActiveMock.mockReturnValue(true);
    isInViewportMock.mockReturnValue(true);
    focusedThreadIdSignal.value = 't-1';
    const { classifyInAppRow } = await importModule();
    expect(classifyInAppRow(target({ thread: 't-1', event: null }))).toBe(
      'row2_or_3_toast_and_badge',
    );
  });

  test('s4_null_event_id_with_other_thread_falls_through_to_toast', async () => {
    isPageActiveMock.mockReturnValue(true);
    focusedThreadIdSignal.value = 't-2';
    const { classifyInAppRow } = await importModule();
    expect(classifyInAppRow(target({ thread: 't-1', event: null }))).toBe(
      'row2_or_3_toast_and_badge',
    );
  });

  test('s4_null_thread_with_event_id_falls_through_to_toast', async () => {
    isPageActiveMock.mockReturnValue(true);
    isInViewportMock.mockReturnValue(true);
    focusedThreadIdSignal.value = 't-1';
    const { classifyInAppRow } = await importModule();
    expect(classifyInAppRow(target({ thread: null, event: 'e-1' }))).toBe(
      'row2_or_3_toast_and_badge',
    );
  });
});

describe('isShellActive', () => {

  test('an ordinary visible shell is active', async () => {
    const { isShellActive } = await importModule();
    expect(isShellActive(true, false, false)).toBe(true);
  });

  test('a hidden shell is inactive whatever the fullscreen state', async () => {
    const { isShellActive } = await importModule();
    expect(isShellActive(false, false, false)).toBe(false);
    expect(isShellActive(false, true, false)).toBe(false);
  });

  test('native fullscreen makes the shell inactive', async () => {
    const { isShellActive } = await importModule();
    expect(isShellActive(true, true, false)).toBe(false);
  });

  test('pseudo-fullscreen makes the shell inactive too', async () => {
    const { isShellActive } = await importModule();
    expect(isShellActive(true, false, true)).toBe(false);
  });
});
