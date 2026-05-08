import { describe, it, expect } from 'vitest';
import { resolveDeepLink } from './notification-deeplink';

describe('resolveDeepLink', () => {
  it('opens the linked app', () => {
    expect(resolveDeepLink({ app: 'my-app' })).toEqual({ type: 'open-app', id: 'my-app' });
  });

  it('opens the notification modal when only notification id is set', () => {
    expect(resolveDeepLink({ notification: 'n-1' })).toEqual({
      type: 'view-notification',
      id: 'n-1',
    });
  });

  it('returns noop when nothing actionable is present', () => {
    expect(resolveDeepLink({})).toEqual({ type: 'noop' });
    expect(resolveDeepLink({ app: null, notification: null })).toEqual({ type: 'noop' });
  });

  it('prefers app over notification when both are set', () => {
    // Push tap on a notification that links an app should land on the app —
    // the inbox modal would just hide the app the user clicked through to.
    expect(resolveDeepLink({ app: 'my-app', notification: 'n-1' })).toEqual({
      type: 'open-app',
      id: 'my-app',
    });
  });

  it('ignores extra unknown fields, including any source thread id', () => {
    // Regression guard: an earlier version focused the source thread on tap,
    // pulling the user away from whatever they were doing. The resolver must
    // never act on a thread id even if a stale SW or push payload includes one.
    const target = { notification: 'n-1', thread: 't-9' } as unknown as Parameters<typeof resolveDeepLink>[0];
    expect(resolveDeepLink(target)).toEqual({ type: 'view-notification', id: 'n-1' });
  });
});
