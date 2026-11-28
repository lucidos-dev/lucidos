import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { DeepLinkTarget } from './notification-deeplink';


const repaintLandedContent = vi.fn<() => void>();
const markReadOptimistic = vi.fn<(id: string, generation?: string) => void>();
const viewNotification = vi.fn<(id: string, generation?: string) => Promise<void>>(async () => {});
const handleNavigationRequest = vi.fn<(nav: { target: string }) => void>();

vi.mock('../../utils/pageResume', async () => {
  const actual = await vi.importActual<typeof import('../../utils/pageResume')>(
    '../../utils/pageResume',
  );
  return { ...actual, repaintLandedContent: () => repaintLandedContent() };
});
vi.mock('./notifications', async () => {
  const actual = await vi.importActual<typeof import('./notifications')>('./notifications');
  return {
    ...actual,
    markReadOptimistic: (id: string, generation?: string) => markReadOptimistic(id, generation),
    viewNotification: (id: string, generation?: string) => viewNotification(id, generation),
  };
});
vi.mock('./navigation-dispatch', async () => {
  const actual = await vi.importActual<typeof import('./navigation-dispatch')>(
    './navigation-dispatch',
  );
  return {
    ...actual,
    handleNavigationRequest: (nav: { target: string }) => handleNavigationRequest(nav),
  };
});

const { dispatchDeepLink } = await import('./in-app-notification-toast');

const NOTIF_ID = '5d8c4d96-8df1-4243-a43d-35449f689bda';

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
    dispatchDeepLink(navigateToApp);
    expect(handleNavigationRequest.mock.invocationCallOrder[0]).toBeLessThan(
      repaintLandedContent.mock.invocationCallOrder[0],
    );
  });

  it('repaints after opening the notification detail', () => {
    expect(dispatchDeepLink({ notification: NOTIF_ID, tap: { kind: 'modal' } })).toBe(true);
    expect(viewNotification).toHaveBeenCalledTimes(1);
    expect(repaintLandedContent).toHaveBeenCalledTimes(1);
  });

  it('does not repaint when the target resolves to nothing', () => {
    expect(dispatchDeepLink({ notification: null, tap: null })).toBe(false);
    expect(repaintLandedContent).not.toHaveBeenCalled();
  });
});

describe('push provenance reaches the read owner', () => {
  it('passes the parsed SW and URL origin through navigate and detail branches', async () => {
    const { parseDeepLinkFromSwMessage, parseDeepLinkFromUrl } = await import('./notification-deeplink');
    for (const generation of ['none', 'unknown', '11111111-1111-4111-8111-111111111111']) {
      const fromSw = parseDeepLinkFromSwMessage({ notification_id: NOTIF_ID,
        restore_generation: generation === 'none' ? null : generation, tap: navigateToApp.tap })!;
      dispatchDeepLink(fromSw);
      expect(markReadOptimistic).toHaveBeenLastCalledWith(NOTIF_ID, generation);
      const fromUrl = parseDeepLinkFromUrl(new URL(
        'https://example.com/?notification=' + NOTIF_ID + '&restore_generation=' + generation));
      dispatchDeepLink(fromUrl);
      expect(viewNotification).toHaveBeenLastCalledWith(NOTIF_ID, generation);
    }
  });
});
