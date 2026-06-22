import { beforeEach, describe, expect, test, vi } from 'vitest';
import type { DeepLinkTarget } from './notification-deeplink';

const isPageActiveMock = vi.fn(() => true);
const isInViewportMock = vi.fn(() => false);
const isIOSPwaMock = vi.fn(() => true);
const focusedThreadIdSignal = { value: null as string | null };

vi.mock('../../utils/pageActive', () => ({ isPageActive: isPageActiveMock }));
vi.mock('../../utils/viewport', () => ({ isInViewport: isInViewportMock }));
vi.mock('../../utils/platform', () => ({ isIOSPwa: isIOSPwaMock }));
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
  isIOSPwaMock.mockReset().mockReturnValue(true);
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

// Phase 0 PWA best-effort deep-link rescue (plan 2026-06-19-ios-native-apns-app).
// On resume the iOS WebKit bug may have swallowed a push tap; the affordance
// surfaces recent UNREAD navigate-kind notifications as a tappable, dismissible
// toast (never an auto-navigation).
describe('surfaceResumeNotificationAffordance (PWA best-effort)', () => {
  const recentIso = () => new Date(Date.now() - 1000).toISOString();
  const oldIso = () => new Date(Date.now() - 25 * 60 * 60 * 1000).toISOString();

  type N = {
    id: string; title: string; message: string; created_at: string;
    read: boolean; tap?: { kind: string; to?: unknown }; thread_id?: string;
  };
  function notif(over: Partial<N> & { id: string }): N {
    return {
      title: 'T', message: 'B', created_at: recentIso(), read: false,
      tap: { kind: 'navigate', to: { target: 'thread', id: 'th-1' } }, ...over,
    };
  }

  async function setUnread(data: N[]) {
    const store = await import('../store');
    store.unreadNotifications.value = { status: 'loaded', data: data as never };
    store.toasts.value = [];
    return store;
  }

  test('recent unread navigate → surfaces one tappable toast (no auto-nav)', async () => {
    const store = await setUnread([notif({ id: 'r-single' })]);
    const { surfaceResumeNotificationAffordance } = await importModule();
    surfaceResumeNotificationAffordance();
    expect(store.toasts.value).toHaveLength(1);
    const t = store.toasts.value[0];
    expect(t.key).toBe('notification-r-single');
    expect(typeof t.onClick).toBe('function'); // tappable; navigation only on tap
  });

  test('read notification is never surfaced', async () => {
    const store = await setUnread([notif({ id: 'r-read', read: true })]);
    const { surfaceResumeNotificationAffordance } = await importModule();
    surfaceResumeNotificationAffordance();
    expect(store.toasts.value).toHaveLength(0);
  });

  test('modal-kind unread is not surfaced (no deep-link to rescue)', async () => {
    const store = await setUnread([notif({ id: 'r-modal', tap: { kind: 'modal' } })]);
    const { surfaceResumeNotificationAffordance } = await importModule();
    surfaceResumeNotificationAffordance();
    expect(store.toasts.value).toHaveLength(0);
  });

  test('unread older than the 24h window is not surfaced', async () => {
    const store = await setUnread([notif({ id: 'r-old', created_at: oldIso() })]);
    const { surfaceResumeNotificationAffordance } = await importModule();
    surfaceResumeNotificationAffordance();
    expect(store.toasts.value).toHaveLength(0);
  });

  test('multiple recent unread → nothing surfaced (no deep-link target, bell badge covers it)', async () => {
    const store = await setUnread([notif({ id: 'r-m1' }), notif({ id: 'r-m2' })]);
    const { surfaceResumeNotificationAffordance } = await importModule();
    surfaceResumeNotificationAffordance();
    expect(store.toasts.value).toHaveLength(0);
  });

  test('2+ unread is not stamped surfaced → a later single fresh unread still gets its rescue toast', async () => {
    // A backlog of 2+ shows nothing but must NOT mark those ids surfaced, or a
    // notification that later becomes the sole fresh unread would be silently
    // skipped — losing the rescue it's entitled to.
    const store = await setUnread([notif({ id: 'r-b1' }), notif({ id: 'r-b2' })]);
    const { surfaceResumeNotificationAffordance } = await importModule();
    surfaceResumeNotificationAffordance();
    expect(store.toasts.value).toHaveLength(0);
    // One gets read elsewhere; only r-b2 remains fresh on the next resume.
    store.unreadNotifications.value = { status: 'loaded', data: [notif({ id: 'r-b2' })] as never };
    surfaceResumeNotificationAffordance();
    expect(store.toasts.value).toHaveLength(1);
    expect(store.toasts.value[0].key).toBe('notification-r-b2');
  });

  test('already-surfaced notification does not re-nag on the next resume', async () => {
    const store = await setUnread([notif({ id: 'r-dedup' })]);
    const { surfaceResumeNotificationAffordance } = await importModule();
    surfaceResumeNotificationAffordance();
    expect(store.toasts.value).toHaveLength(1);
    store.toasts.value = []; // simulate the toast being dismissed before the next wake
    surfaceResumeNotificationAffordance();
    expect(store.toasts.value).toHaveLength(0);
  });

  test('a notification the deep link already handled is not also surfaced (no nav+toast double)', async () => {
    // When an iOS tap DID deep-link, dispatchDeepLink stamps the notification via
    // markNotificationSurfaced. Even if the affordance's unread reload still shows
    // it unread (the mark-read POST hasn't landed — the race that produced a
    // redundant toast on top of the navigation), the surfaced stamp skips it.
    const { surfaceResumeNotificationAffordance, markNotificationSurfaced } = await importModule();
    markNotificationSurfaced('r-handled');
    const store = await setUnread([notif({ id: 'r-handled' })]);
    surfaceResumeNotificationAffordance();
    expect(store.toasts.value).toHaveLength(0);
  });

  test('no-op on a non-iOS-PWA platform (desktop browser gets no resume toast)', async () => {
    isIOSPwaMock.mockReturnValue(false);
    const store = await setUnread([notif({ id: 'r-desktop' })]);
    const { surfaceResumeNotificationAffordance } = await importModule();
    surfaceResumeNotificationAffordance();
    expect(store.toasts.value).toHaveLength(0);
  });
});
