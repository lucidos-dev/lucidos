import { describe, it, expect } from 'vitest';
import {
  resolveDeepLink,
  parseDeepLinkFromUrl,
  parseDeepLinkFromSwMessage,
  hasDeepLinkParams,
  stripDeepLinkFromUrl,
} from './notification-deeplink';

describe('resolveDeepLink', () => {
  describe('modal kind — view-notification', () => {
    it('opens the inbox modal for tap.kind=modal with a notification id', () => {
      expect(
        resolveDeepLink({ notification: 'n-1', tap: { kind: 'modal' } }),
      ).toEqual({ type: 'view-notification', id: 'n-1' });
    });

    it('defaults to modal when tap is null (legacy URL without structured tap)', () => {
      expect(resolveDeepLink({ notification: 'n-1', tap: null })).toEqual({
        type: 'view-notification',
        id: 'n-1',
      });
    });

    it('defaults to modal when tap is undefined', () => {
      expect(resolveDeepLink({ notification: 'n-1' })).toEqual({
        type: 'view-notification',
        id: 'n-1',
      });
    });

    it('noop when tap.kind=modal with no notification id (nothing to open)', () => {
      expect(resolveDeepLink({ tap: { kind: 'modal' } })).toEqual({ type: 'noop' });
    });
  });

  describe('none kind — mark-read only', () => {
    it('marks read when tap.kind=none and a notification id is present', () => {
      expect(
        resolveDeepLink({ notification: 'n-1', tap: { kind: 'none' } }),
      ).toEqual({ type: 'mark-read', id: 'n-1' });
    });

    it('ignores context fields — none never navigates', () => {
      expect(
        resolveDeepLink({
          notification: 'n-1',
          thread: 't-9',
          event: 'e-7',
          tap: { kind: 'none' },
        }),
      ).toEqual({ type: 'mark-read', id: 'n-1' });
    });

    it('noop when tap.kind=none arrives without a notification id', () => {
      expect(resolveDeepLink({ tap: { kind: 'none' } })).toEqual({ type: 'noop' });
    });
  });

  describe('navigate kind — forwards to NavigateUi router', () => {
    it('forwards the full NavigateUi payload to the navigate action (thread + event_id)', () => {
      expect(
        resolveDeepLink({
          notification: 'n-1',
          tap: { kind: 'navigate', to: { target: 'thread', id: 't-9', event_id: 'e-7' } },
        }),
      ).toEqual({
        type: 'navigate',
        to: { target: 'thread', id: 't-9', event_id: 'e-7' },
        notification: 'n-1',
      });
    });

    it('preserves notification id so the dispatcher can mark-read in parallel', () => {
      const result = resolveDeepLink({
        notification: 'n-42',
        tap: { kind: 'navigate', to: { target: 'app', app_id: 'habit-tracker' } },
      });
      expect(result).toEqual({
        type: 'navigate',
        to: { target: 'app', app_id: 'habit-tracker' },
        notification: 'n-42',
      });
    });

    it('navigate with a panel target (no id) still forwards', () => {
      expect(
        resolveDeepLink({ tap: { kind: 'navigate', to: { target: 'changes' } } }),
      ).toEqual({
        type: 'navigate',
        to: { target: 'changes' },
        notification: null,
      });
    });

    it('navigate without a source notification yields notification: null', () => {
      expect(
        resolveDeepLink({ tap: { kind: 'navigate', to: { target: 'settings', settings_view: 'devices' } } }),
      ).toEqual({
        type: 'navigate',
        to: { target: 'settings', settings_view: 'devices' },
        notification: null,
      });
    });
  });

  describe('edge cases', () => {
    it('returns noop when nothing actionable is present', () => {
      expect(resolveDeepLink({})).toEqual({ type: 'noop' });
      expect(resolveDeepLink({ notification: null, thread: null })).toEqual({
        type: 'noop',
      });
    });
  });
});

describe('parseDeepLinkFromUrl', () => {
  it('reads context params (notification, thread, event) from the URL hash', () => {
    // Service worker writes deep-link as a hash so warm-page navigate is a
    // hash-only change (no reload, no "site updated" toast).
    const url = new URL(
      'https://localhost:5174/#notification=n-1&thread=t-9&event=e-7',
    );
    expect(parseDeepLinkFromUrl(url)).toEqual({
      notification: 'n-1',
      thread: 't-9',
      event: 'e-7',
      tap: null,
    });
  });

  it('reads params from the query string when hash is empty', () => {
    // Legacy / direct-link case: older SW versions and any human-typed deep
    // link still arrive as `?notification=...&thread=...`.
    const url = new URL('https://localhost:5174/?notification=n-1&thread=t-9');
    expect(parseDeepLinkFromUrl(url)).toEqual({
      notification: 'n-1',
      thread: 't-9',
      event: null,
      tap: null,
    });
  });

  it('prefers hash over query when both are present', () => {
    // After an iOS PWA cold-start, the SW may have set the hash on a URL that
    // also retains stale query params from a previous session. Hash is the
    // current intent.
    const url = new URL(
      'https://localhost:5174/?notification=stale#notification=fresh&thread=t-9',
    );
    expect(parseDeepLinkFromUrl(url)).toMatchObject({
      notification: 'fresh',
      thread: 't-9',
    });
  });

  it('decodes a JSON-encoded structured tap from the URL hash', () => {
    // The SW writes the structured `Tap` as JSON in the `tap=` URL param so
    // cold-tab opens can recover navigate-kind taps. The `app=` legacy key
    // is gone — `tap.to.app_id` is the source of truth for navigate targets.
    const tap = { kind: 'navigate', to: { target: 'thread', id: 't-9', event_id: 'e-7' } };
    const url = new URL(
      `https://localhost:5174/#notification=n-1&thread=t-9&event=e-7&tap=${encodeURIComponent(JSON.stringify(tap))}`,
    );
    expect(parseDeepLinkFromUrl(url)).toEqual({
      notification: 'n-1',
      thread: 't-9',
      event: 'e-7',
      tap,
    });
  });

  it('drops a stale legacy `tap=open_thread` URL string (pre-migration leftover)', () => {
    // A URL written by an older SW build (before the JSON-encoded tap shape)
    // still carries `tap=open_thread`. `validateTap` rejects bare strings,
    // so the parser yields `tap: null` and the dispatcher safely demotes to
    // the modal default.
    const url = new URL('https://localhost:5174/?app=my-app&tap=open_app#notification=n-1');
    const target = parseDeepLinkFromUrl(url);
    expect(target).toEqual({
      notification: 'n-1',
      thread: null,
      event: null,
      tap: null,
    });
  });

  it('drops a malformed JSON tap value', () => {
    const url = new URL('https://localhost:5174/#notification=n-1&tap=%7Bnot-json');
    expect(parseDeepLinkFromUrl(url).tap).toBeNull();
  });

  it('returns nulls when neither hash nor query has deep-link params', () => {
    const url = new URL('https://localhost:5174/');
    expect(parseDeepLinkFromUrl(url)).toEqual({
      notification: null,
      thread: null,
      event: null,
      tap: null,
    });
  });

  it('surfaces the bare #thread=UUID channel (cross-workspace landing)', () => {
    // `openThreadInWorkspace` ships users to `#thread=<uuid>` without a
    // `notification=` key. THREAD_HASH_RE in useStartup owns that shape and
    // runs first — `parseDeepLinkFromUrl` still surfaces the thread id, but
    // a notification deep-link without a `notification` key is moot.
    const url = new URL('https://localhost:5174/#thread=abc-123');
    const target = parseDeepLinkFromUrl(url);
    expect(target.thread).toBe('abc-123');
    expect(target.notification).toBeNull();
  });
});

describe('parseDeepLinkFromSwMessage', () => {
  // The service worker posts the raw `tapData` shape it built from the push
  // payload — keys are `notification_id` / `thread_id` / `event_id` / `tap`.
  // The page's DeepLinkTarget uses the trimmed names. This converter is the
  // single source of truth for that translation; the message handler wires
  // it up. `tap` arrives as a structured object (postMessage handles objects
  // natively — no JSON round-trip).
  it('translates the SW tapData shape with a structured navigate tap', () => {
    expect(
      parseDeepLinkFromSwMessage({
        notification_id: 'n-1',
        thread_id: 't-9',
        event_id: 'e-7',
        tap: { kind: 'navigate', to: { target: 'thread', id: 't-9', event_id: 'e-7' } },
      }),
    ).toEqual({
      notification: 'n-1',
      thread: 't-9',
      event: 'e-7',
      tap: { kind: 'navigate', to: { target: 'thread', id: 't-9', event_id: 'e-7' } },
    });
  });

  it('accepts tap.kind=modal and tap.kind=none', () => {
    expect(
      parseDeepLinkFromSwMessage({ notification_id: 'n-1', tap: { kind: 'modal' } })?.tap,
    ).toEqual({ kind: 'modal' });
    expect(
      parseDeepLinkFromSwMessage({ notification_id: 'n-1', tap: { kind: 'none' } })?.tap,
    ).toEqual({ kind: 'none' });
  });

  it('fills in nulls for missing keys', () => {
    expect(parseDeepLinkFromSwMessage({ notification_id: 'n-1' })).toEqual({
      notification: 'n-1',
      thread: null,
      event: null,
      tap: null,
    });
  });

  it('drops tap objects with an unknown kind', () => {
    expect(
      parseDeepLinkFromSwMessage({ notification_id: 'n-1', tap: { kind: 'open_anywhere' } })?.tap,
    ).toBeNull();
  });

  it('drops a bare string tap (old SW version sending pre-migration shape)', () => {
    expect(
      parseDeepLinkFromSwMessage({ notification_id: 'n-1', tap: 'open_thread' })?.tap,
    ).toBeNull();
  });

  it('drops a navigate tap with a missing or non-object `to`', () => {
    expect(
      parseDeepLinkFromSwMessage({ notification_id: 'n-1', tap: { kind: 'navigate' } })?.tap,
    ).toBeNull();
    expect(
      parseDeepLinkFromSwMessage({ notification_id: 'n-1', tap: { kind: 'navigate', to: 'thread' } })?.tap,
    ).toBeNull();
  });

  it('drops a navigate tap with no string `target` in `to`', () => {
    expect(
      parseDeepLinkFromSwMessage({ notification_id: 'n-1', tap: { kind: 'navigate', to: {} } })?.tap,
    ).toBeNull();
  });

  it('returns null for non-object payloads', () => {
    expect(parseDeepLinkFromSwMessage(null)).toBeNull();
    expect(parseDeepLinkFromSwMessage(undefined)).toBeNull();
    expect(parseDeepLinkFromSwMessage('string')).toBeNull();
    expect(parseDeepLinkFromSwMessage(42)).toBeNull();
  });

  it('coerces non-string ids to null', () => {
    expect(
      parseDeepLinkFromSwMessage({
        notification_id: 42,
        thread_id: undefined,
      }),
    ).toEqual({
      notification: null,
      thread: null,
      event: null,
      tap: null,
    });
  });
});

describe('hasDeepLinkParams', () => {
  it('is true when notification is set', () => {
    expect(hasDeepLinkParams({ notification: 'n-1' })).toBe(true);
  });
  it('is false when only thread is set (no notification to dispatch)', () => {
    // Bare `#thread=<uuid>` is THREAD_HASH_RE's job, not the deep-link
    // dispatcher's. Without `notification`, resolveDeepLink would noop —
    // claiming it's dispatch-worthy would cause stripDeepLinkFromUrl to
    // clear the URL with nothing replacing the navigation.
    expect(hasDeepLinkParams({ thread: 't-9' })).toBe(false);
  });
  it('is false on an empty target', () => {
    expect(hasDeepLinkParams({})).toBe(false);
    expect(hasDeepLinkParams({ notification: null, thread: null })).toBe(false);
  });
});

describe('stripDeepLinkFromUrl', () => {
  it('removes both hash and query deep-link keys, preserving the rest', () => {
    // After dispatching the deep link we strip the params so a refresh doesn't
    // re-fire the navigation.
    const url = new URL(
      'https://localhost:5174/?other=keep&notification=n-1#notification=n-1&thread=t-9&foo=bar',
    );
    const cleaned = stripDeepLinkFromUrl(url);
    expect(cleaned.search).toBe('?other=keep');
    // Non-deep-link hash entries (rare, but possible) are preserved.
    expect(cleaned.hash).toBe('#foo=bar');
  });

  it('clears the hash entirely when every key was deep-link-related', () => {
    const url = new URL('https://localhost:5174/#notification=n-1&thread=t-9&event=e-7');
    expect(stripDeepLinkFromUrl(url).hash).toBe('');
  });

  it('strips a bare #thread=UUID once cross-workspace handling has passed it on', () => {
    // useStartup runs THREAD_HASH_RE first; whatever reaches strip is either
    // the SW deep-link (notification + ...) or stale state safe to remove.
    const url = new URL('https://localhost:5174/#thread=abc-123');
    expect(stripDeepLinkFromUrl(url).hash).toBe('');
  });

  it('strips the JSON-encoded tap param after dispatch', () => {
    // `tap` is in DEEP_LINK_KEYS so a refresh doesn't re-fire the navigation.
    const tap = { kind: 'navigate', to: { target: 'thread', id: 't-9' } };
    const url = new URL(
      `https://localhost:5174/?other=keep#notification=n-1&tap=${encodeURIComponent(JSON.stringify(tap))}`,
    );
    const cleaned = stripDeepLinkFromUrl(url);
    expect(cleaned.search).toBe('?other=keep');
    expect(cleaned.hash).toBe('');
  });

  it('leaves the dropped legacy `app` key alone (no longer ours to manage)', () => {
    // `app` was dropped from DEEP_LINK_KEYS (the structured tap carries
    // `to.app_id` instead). Stale `app=` from older SW URLs is preserved.
    const url = new URL('https://localhost:5174/?app=stale');
    const cleaned = stripDeepLinkFromUrl(url);
    expect(cleaned.search).toBe('?app=stale');
  });
});
