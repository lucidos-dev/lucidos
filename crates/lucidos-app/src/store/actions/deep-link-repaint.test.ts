import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { DeepLinkTarget } from './notification-deeplink';

// A deep link lands its view on a page that may not have painted since it woke.
//
// A native banner only shows while the page is inactive, so a banner tap always
// arrives on a webview WebKit has parked. The client recovers a parked layer on
// the wake, but the link itself lands later: the drain is an IPC round trip, and
// `openAppById` may await the apps list, and `AppUiInline` is a lazy chunk. So
// the wake repaint runs against the view the tap is replacing, and the arriving
// one is never repainted. That shipped as an app deep link whose iframe loaded
// while the screen kept the pre-tap frame.
//
// `dispatchDeepLink` is the one router every surface uses, so it is the one
// place that knows content just landed. See the plan
// docs/plans/2026-09-17-a-deep-link-repaints-what-it-landed.md.

const repaintLandedContent = vi.fn<() => void>();
const markReadOptimistic = vi.fn<(id: string) => void>();
const viewNotification = vi.fn<(id: string) => Promise<void>>(async () => {});
const handleNavigationRequest = vi.fn<(nav: { target: string }) => void>();

vi.mock('../../utils/pageResume', async () => {
  const actual = await vi.importActual<typeof import('../../utils/pageResume')>(
    '../../utils/pageResume',
  );
  return { ...actual, repaintLandedContent: () => repaintLandedContent() };
});
// Partial mocks throughout: the toast module pulls other exports of each of
// these in, and a narrow stub would break them at import.
vi.mock('./notifications', async () => {
  const actual = await vi.importActual<typeof import('./notifications')>('./notifications');
  return {
    ...actual,
    markReadOptimistic: (id: string) => markReadOptimistic(id),
    viewNotification: (id: string) => viewNotification(id),
  };
});
vi.mock('./navigation-request', async () => {
  const actual = await vi.importActual<typeof import('./navigation-request')>(
    './navigation-request',
  );
  return {
    ...actual,
    handleNavigationRequest: (nav: { target: string }) => handleNavigationRequest(nav),
  };
});

const { dispatchDeepLink } = await import('./in-app-notification-toast');

const NOTIF_ID = '5d8c4d96-8df1-4243-a43d-35449f689bda';

/** The tap the reported bug carried: a banner pointing at an app. */
const navigateToApp: DeepLinkTarget = {
  notification: NOTIF_ID,
  thread: null,
  event: null,
  tap: { kind: 'navigate', to: { target: 'app', app_id: 'habit-tracker', fragment: 'today' } },
};

describe('a deep link repaints what it landed', () => {
  beforeEach(() => {
    repaintLandedContent.mockClear();
    markReadOptimistic.mockClear();
    viewNotification.mockClear();
    handleNavigationRequest.mockClear();
  });

  it('repaints after routing a navigate tap', () => {
    expect(dispatchDeepLink(navigateToApp)).toBe(true);
    expect(handleNavigationRequest).toHaveBeenCalledTimes(1);
    expect(repaintLandedContent).toHaveBeenCalledTimes(1);
  });

  it('repaints AFTER the navigation, never before it', () => {
    // Order is the whole point. A repaint issued before the view is asked for
    // lands on the outgoing one, which is the wake repaint's own failure.
    dispatchDeepLink(navigateToApp);
    expect(handleNavigationRequest.mock.invocationCallOrder[0]).toBeLessThan(
      repaintLandedContent.mock.invocationCallOrder[0],
    );
  });

  it('repaints after opening the notification detail', () => {
    // The `modal` default lands the detail in the same pane, off the same tap,
    // so it meets the same parked layer.
    expect(dispatchDeepLink({ notification: NOTIF_ID, tap: { kind: 'modal' } })).toBe(true);
    expect(viewNotification).toHaveBeenCalledTimes(1);
    expect(repaintLandedContent).toHaveBeenCalledTimes(1);
  });

  it('does not repaint when the target resolves to nothing', () => {
    // No notification and no tap: nothing was landed, so there is nothing to
    // paint and no reason to touch the compositor.
    expect(dispatchDeepLink({ notification: null, tap: null })).toBe(false);
    expect(repaintLandedContent).not.toHaveBeenCalled();
  });
});
