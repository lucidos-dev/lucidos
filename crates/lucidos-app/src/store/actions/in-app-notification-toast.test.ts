import { beforeEach, describe, expect, test, vi } from 'vitest';
import type { DeepLinkTarget } from './notification-deeplink';

const isPageActiveMock = vi.fn(() => true);
const isInViewportMock = vi.fn(() => false);
const focusedThreadIdSignal = { value: null as string | null };

vi.mock('../../utils/pageActive', () => ({ isPageActive: isPageActiveMock }));
vi.mock('../../utils/viewport', () => ({ isInViewport: isInViewportMock }));
vi.mock('../store', async () => {
  // Pull in the rest of the store so the toast module's other imports
  // (showToast, dismissToast, toasts) still resolve.
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

// §5.1 — every matrix row in system-knowhow/notifications.md §4. Row IDs
// match the spec section labels so a failing test points at the broken
// row.

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

  test('s4_row4_hidden_classifies_as_hidden_regardless_of_focus_or_viewport', async () => {
    // Even when focused on the source thread and the event is "in viewport"
    // (hidden tab could still report bbox), an inactive page never gets a
    // toast and never auto-marks-read.
    isPageActiveMock.mockReturnValue(false);
    isInViewportMock.mockReturnValue(true);
    focusedThreadIdSignal.value = 't-1';
    const { classifyInAppRow } = await importModule();
    expect(classifyInAppRow(target({ thread: 't-1', event: 'e-1' }))).toBe('row4_hidden');
  });

  test('s4_null_event_id_with_focused_thread_falls_through_to_toast', async () => {
    // Spec §2: Row 1 requires non-null event_id. Same-thread notification
    // without one drops into Row 2 (toast + badge).
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
    // No thread to focus → Row 1's "on source thread" predicate fails.
    isPageActiveMock.mockReturnValue(true);
    isInViewportMock.mockReturnValue(true);
    focusedThreadIdSignal.value = 't-1';
    const { classifyInAppRow } = await importModule();
    expect(classifyInAppRow(target({ thread: null, event: 'e-1' }))).toBe(
      'row2_or_3_toast_and_badge',
    );
  });
});
