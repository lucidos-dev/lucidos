import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';

// Capture all calls to dependencies so we can assert the pong body shape.
const isPageActiveMock = vi.fn(() => true);
const isInViewportMock = vi.fn(() => false);
const getDeviceIdMock = vi.fn(() => 'dev-test');
const focusedThreadIdSignal = { value: null as string | null };
// Regression guard: PresenceCheck must NEVER render a toast anymore — the
// toast moved to NotificationToastRequested so it can't race the push
// decision. presence-pong.ts no longer imports this; the mock just lets us
// assert it's never reached.
const showInAppNotificationToastMock = vi.fn();

vi.mock('../../api/client', () => ({ API_BASE: 'http://test', API: 'http://test/api/v1' }));
vi.mock('../../utils/pageActive', () => ({ isPageActive: isPageActiveMock }));
vi.mock('../../utils/viewport', () => ({ isInViewport: isInViewportMock }));
vi.mock('../store', () => ({ focusedThreadId: focusedThreadIdSignal }));
vi.mock('./devices', () => ({ getDeviceId: getDeviceIdMock }));
vi.mock('./in-app-notification-toast', () => ({
  showInAppNotificationToast: showInAppNotificationToastMock,
  handleNotificationToastRequested: vi.fn(),
}));

// Lazy-import after mocks are set up.
const importModule = async () => await import('./presence-pong');

describe('handlePresenceCheck', () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    fetchMock = vi.fn(() => Promise.resolve(new Response(null, { status: 200 })));
    vi.stubGlobal('fetch', fetchMock);
    isPageActiveMock.mockReset().mockReturnValue(true);
    isInViewportMock.mockReset().mockReturnValue(false);
    getDeviceIdMock.mockReset().mockReturnValue('dev-test');
    focusedThreadIdSignal.value = null;
    showInAppNotificationToastMock.mockReset();
  });

  /** Wall-clock ms — handlers compare `sent_at_ms` against `Date.now()`. */
  const now = () => Date.now();

  afterEach(() => {
    vi.unstubAllGlobals();
  });

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
    expect(fetchMock).toHaveBeenCalledOnce();
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe('http://test/api/v1/presence-pong');
    expect(init.method).toBe('POST');
    expect(init.keepalive).toBe(true);
    const body = JSON.parse(init.body as string);
    expect(body).toEqual({
      notification_id: 'n-1',
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
    const [, init] = fetchMock.mock.calls[0];
    const body = JSON.parse(init.body as string);
    expect(body.event_in_viewport).toBe(false);
    expect(isInViewportMock).not.toHaveBeenCalled();
  });

  test('s3_pong_is_fire_and_forget_swallows_network_error', async () => {
    // Spec §3 "Failure handling" — a missed pong is treated as not-active.
    // The handler must not throw, since SSE dispatch can't recover.
    fetchMock.mockRejectedValue(new Error('network down'));
    const { handlePresenceCheck } = await importModule();
    expect(() =>
      handlePresenceCheck({
        notification_id: 'n-1',
        event_id: null,
        deadline_ms: 250,
        sent_at_ms: now(),
      }),
    ).not.toThrow();
    // Give the .catch handler a chance to run.
    await new Promise((r) => setTimeout(r, 0));
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
    expect(fetchMock).not.toHaveBeenCalled();
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
    expect(fetchMock).toHaveBeenCalledOnce();
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
    expect(fetchMock).toHaveBeenCalledOnce();
    const [, init] = fetchMock.mock.calls[0];
    expect(JSON.parse(init.body as string).is_active).toBe(false);
    expect(showInAppNotificationToastMock).not.toHaveBeenCalled();
  });
});
