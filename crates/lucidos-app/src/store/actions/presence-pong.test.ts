import { beforeEach, describe, expect, test, vi } from 'vitest';

// Capture all calls to dependencies so we can assert the pong answer shape.
const isPageActiveMock = vi.fn(() => true);
const isInViewportMock = vi.fn(() => false);
const getDeviceIdMock = vi.fn(() => 'dev-test');
const focusedThreadIdSignal = { value: null as string | null };
const pseudoFullscreenSignal = { value: false };
const fullscreenHostSignal = { value: null as unknown };
// The pong now goes through the TRANSPORT, not straight to fetch. On a shared
// connection the worker ORs this answer with its peers' answers. It then POSTs
// one pong, so the engine still gets one per open stream.
const submitPongMock = vi.fn();
// Regression guard: PresenceCheck must NEVER render a toast anymore — the
// toast moved to NotificationToastRequested so it can't race the push
// decision. presence-pong.ts no longer imports this; the mock just lets us
// assert it's never reached.
const showInAppNotificationToastMock = vi.fn();

vi.mock('../../api/client', () => ({ API_BASE: 'http://test', API: 'http://test/api/v1' }));
vi.mock('../../utils/pageActive', () => ({ isPageActive: isPageActiveMock }));
vi.mock('../../utils/viewport', () => ({ isInViewport: isInViewportMock }));
vi.mock('../store', () => ({
  focusedThreadId: focusedThreadIdSignal,
  appPseudoFullscreen: pseudoFullscreenSignal,
}));
vi.mock('../appFullscreenHost', () => ({ appFullscreenHost: fullscreenHostSignal }));
vi.mock('./devices', () => ({ getDeviceId: getDeviceIdMock, pendingDeviceRegistration: vi.fn() }));
vi.mock('./event-stream', () => ({ submitPong: submitPongMock }));
vi.mock('./in-app-notification-toast', () => ({
  showInAppNotificationToast: showInAppNotificationToastMock,
  handleNotificationToastRequested: vi.fn(),
}));

// Lazy-import after mocks are set up.
const importModule = async () => await import('./presence-pong');

describe('handlePresenceCheck', () => {
  beforeEach(() => {
    isPageActiveMock.mockReset().mockReturnValue(true);
    isInViewportMock.mockReset().mockReturnValue(false);
    getDeviceIdMock.mockReset().mockReturnValue('dev-test');
    focusedThreadIdSignal.value = null;
    pseudoFullscreenSignal.value = false;
    fullscreenHostSignal.value = null;
    submitPongMock.mockReset();
    showInAppNotificationToastMock.mockReset();
  });

  /** Wall-clock ms — handlers compare `sent_at_ms` against `Date.now()`. */
  const now = () => Date.now();

  test('s3_pong_body_carries_device_state_and_event_in_viewport', async () => {
    isPageActiveMock.mockReturnValue(true);
    isInViewportMock.mockReturnValue(true);
    focusedThreadIdSignal.value = 't-1';
    const { handlePresenceCheck } = await importModule();
    handlePresenceCheck({
      notification_id: 'n-1',
      event_id: 'e-1',
      deadline_ms: 250,
      sent_at_ms: now(),
    });
    expect(submitPongMock).toHaveBeenCalledOnce();
    const [notificationId, answer] = submitPongMock.mock.calls[0];
    expect(notificationId).toBe('n-1');
    expect(answer).toEqual({
      device_id: 'dev-test',
      is_active: true,
      focused_thread_id: 't-1',
      event_in_viewport: true,
    });
    expect(isInViewportMock).toHaveBeenCalledWith('e-1');
    // PresenceCheck is a pure pong trigger — no toast here.
    expect(showInAppNotificationToastMock).not.toHaveBeenCalled();
  });

  test('s3_pong_event_in_viewport_is_false_when_event_id_null', async () => {
    // Spec §3 — null event_id always pongs event_in_viewport=false without
    // even calling the helper (no DOM lookup needed).
    isInViewportMock.mockReturnValue(true);
    const { handlePresenceCheck } = await importModule();
    handlePresenceCheck({
      notification_id: 'n-1',
      event_id: null,
      deadline_ms: 250,
      sent_at_ms: now(),
    });
    expect(submitPongMock.mock.calls[0][1].event_in_viewport).toBe(false);
    expect(isInViewportMock).not.toHaveBeenCalled();
  });

  // §3 freshness check — iOS PWA buffers SSE messages while JS is suspended in
  // the background. When the user taps the push, the queued PresenceCheck
  // fires AFTER the engine's deadline has long passed. A late pong is dropped
  // by the engine anyway, so skip it — keeps the engine's pong accounting clean.
  test('s3_stale_presence_check_drops_no_pong', async () => {
    isPageActiveMock.mockReturnValue(true);
    const { handlePresenceCheck, STALE_GRACE_MS } = await importModule();
    handlePresenceCheck({
      notification_id: 'n-late',
      event_id: 'e-1',
      deadline_ms: 250,
      sent_at_ms: now() - 250 - STALE_GRACE_MS - 100,
    });
    expect(submitPongMock).not.toHaveBeenCalled();
    expect(showInAppNotificationToastMock).not.toHaveBeenCalled();
  });

  test('s3_fresh_presence_check_within_grace_pongs_but_does_not_toast', async () => {
    // The whole point of the redesign: even a fresh, active PresenceCheck only
    // pongs. The toast is the engine's call (NotificationToastRequested), made
    // AFTER it has the pong — so the toast can never race ahead of the push
    // decision and double up.
    isPageActiveMock.mockReturnValue(true);
    focusedThreadIdSignal.value = 't-2';
    const { handlePresenceCheck } = await importModule();
    handlePresenceCheck({
      notification_id: 'n-fresh',
      event_id: 'e-1',
      deadline_ms: 250,
      sent_at_ms: now() - 100, // well inside deadline + grace
    });
    expect(submitPongMock).toHaveBeenCalledOnce();
    expect(showInAppNotificationToastMock).not.toHaveBeenCalled();
  });

  // §4 row 4: page inactive (hidden) pongs is_active=false so the engine knows
  // to send the push. No toast either way — PresenceCheck never toasts now.
  test('s4_via_presence_check_inactive_page_pongs_no_toast', async () => {
    isPageActiveMock.mockReturnValue(false);
    const { handlePresenceCheck } = await importModule();
    handlePresenceCheck({
      notification_id: 'n-1',
      event_id: 'e-1',
      deadline_ms: 250,
      sent_at_ms: now(),
    });
    expect(submitPongMock).toHaveBeenCalledOnce();
    expect(submitPongMock.mock.calls[0][1].is_active).toBe(false);
    expect(showInAppNotificationToastMock).not.toHaveBeenCalled();
  });

  test('a fullscreen app reports inactive, so the notification takes the OS', async () => {
    isPageActiveMock.mockReturnValue(true);
    fullscreenHostSignal.value = {} as unknown;
    const { handlePresenceCheck } = await importModule();
    handlePresenceCheck({
      notification_id: 'n-1',
      event_id: null,
      deadline_ms: 250,
      sent_at_ms: now(),
    });
    expect(submitPongMock.mock.calls[0][1].is_active).toBe(false);
  });
});

describe('isShellActive', () => {
  // Presence answers whether the SHELL can show a toast. An app filling the
  // screen means the user is looking at the app, so the OS surface wins. That
  // also makes a fullscreen app agree with a popped-out app window, which has
  // always taken the push and looks identical to the user.

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
    // The iOS path is a CSS overlay rather than the Fullscreen API, so it sets
    // a different signal. Both mean the same thing to the user.
    const { isShellActive } = await importModule();
    expect(isShellActive(true, false, true)).toBe(false);
  });

  test('an app merely open in the content pane leaves the shell active', async () => {
    // The common case, and it must not regress: an app open but not fullscreen
    // still has the shell around it to render a toast.
    const { isShellActive } = await importModule();
    expect(isShellActive(true, false, false)).toBe(true);
  });
});
