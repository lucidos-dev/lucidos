import { describe, it, expect } from 'vitest';
import type { Tap } from '@lucidos/sdk';
import {
  resolveDeepLink,
  parseDeepLinkFromUrl,
  parseDeepLinkFromSwMessage,
  parseDeepLinkFromInboxRow,
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

  describe('none kind — retired, coerces to the openable modal path', () => {
    const noneTap = { kind: 'none' } as unknown as Tap;

    it('coerces a historical tap.kind=none (with a notification id) to view-notification', () => {
      expect(
        resolveDeepLink({ notification: 'n-1', tap: noneTap }),
      ).toEqual({ type: 'view-notification', id: 'n-1' });
    });

    it('coerces none to view-notification even with context fields (never navigates)', () => {
      expect(
        resolveDeepLink({
          notification: 'n-1',
          thread: 't-9',
          event: 'e-7',
          tap: noneTap,
        }),
      ).toEqual({ type: 'view-notification', id: 'n-1' });
    });

    it('noop when a historical none arrives without a notification id (nothing to open)', () => {
      expect(resolveDeepLink({ tap: noneTap })).toEqual({ type: 'noop' });
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
    const url = new URL(
      'https://localhost:5174/#notification=n-1&thread=t-9&event=e-7',
    );
    expect(parseDeepLinkFromUrl(url)).toEqual({
      notification: 'n-1',
      restoreGeneration: 'unknown',
      thread: 't-9',
      event: 'e-7',
      tap: null,
    });
  });

  it('reads params from the query string when hash is empty', () => {
    const url = new URL('https://localhost:5174/?notification=n-1&thread=t-9');
    expect(parseDeepLinkFromUrl(url)).toEqual({
      notification: 'n-1',
      restoreGeneration: 'unknown',
      thread: 't-9',
      event: null,
      tap: null,
    });
  });

  it('prefers hash over query when both are present', () => {
    const url = new URL(
      'https://localhost:5174/?notification=stale#notification=fresh&thread=t-9',
    );
    expect(parseDeepLinkFromUrl(url)).toMatchObject({
      notification: 'fresh',
      restoreGeneration: 'unknown',
      thread: 't-9',
    });
  });

  it('decodes a JSON-encoded structured tap from the URL hash', () => {
    const tap = { kind: 'navigate', to: { target: 'thread', id: 't-9', event_id: 'e-7' } };
    const url = new URL(
      `https://localhost:5174/#notification=n-1&thread=t-9&event=e-7&tap=${encodeURIComponent(JSON.stringify(tap))}`,
    );
    expect(parseDeepLinkFromUrl(url)).toEqual({
      notification: 'n-1',
      restoreGeneration: 'unknown',
      thread: 't-9',
      event: 'e-7',
      tap,
    });
  });

  it('drops a stale legacy `tap=open_thread` URL string (pre-migration leftover)', () => {
    const url = new URL('https://localhost:5174/?app=my-app&tap=open_app#notification=n-1');
    const target = parseDeepLinkFromUrl(url);
    expect(target).toEqual({
      notification: 'n-1',
      restoreGeneration: 'unknown',
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
      restoreGeneration: 'unknown',
      thread: null,
      event: null,
      tap: null,
    });
  });

  it('surfaces the bare #thread=UUID channel (cross-workspace landing)', () => {
    const url = new URL('https://localhost:5174/#thread=abc-123');
    const target = parseDeepLinkFromUrl(url);
    expect(target.thread).toBe('abc-123');
    expect(target.notification).toBeNull();
  });
});

describe('parseDeepLinkFromSwMessage', () => {
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
      restoreGeneration: 'unknown',
      thread: 't-9',
      event: 'e-7',
      tap: { kind: 'navigate', to: { target: 'thread', id: 't-9', event_id: 'e-7' } },
    });
  });

  it('accepts tap.kind=modal and DROPS the retired tap.kind=none', () => {
    expect(
      parseDeepLinkFromSwMessage({ notification_id: 'n-1', tap: { kind: 'modal' } })?.tap,
    ).toEqual({ kind: 'modal' });
    expect(
      parseDeepLinkFromSwMessage({ notification_id: 'n-1', tap: { kind: 'none' } })?.tap,
    ).toBeNull();
  });

  it('fills in nulls for missing keys', () => {
    expect(parseDeepLinkFromSwMessage({ notification_id: 'n-1' })).toEqual({
      notification: 'n-1',
      restoreGeneration: 'unknown',
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
      restoreGeneration: 'unknown',
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
    expect(hasDeepLinkParams({ thread: 't-9' })).toBe(false);
  });
  it('is false on an empty target', () => {
    expect(hasDeepLinkParams({})).toBe(false);
    expect(hasDeepLinkParams({ notification: null, thread: null })).toBe(false);
  });
});

describe('stripDeepLinkFromUrl', () => {
  it('removes both hash and query deep-link keys, preserving the rest', () => {
    const url = new URL(
      'https://localhost:5174/?other=keep&notification=n-1#notification=n-1&thread=t-9&foo=bar',
    );
    const cleaned = stripDeepLinkFromUrl(url);
    expect(cleaned.search).toBe('?other=keep');
    expect(cleaned.hash).toBe('#foo=bar');
  });

  it('clears the hash entirely when every key was deep-link-related', () => {
    const url = new URL('https://localhost:5174/#notification=n-1&thread=t-9&event=e-7');
    expect(stripDeepLinkFromUrl(url).hash).toBe('');
  });

  it('strips a bare #thread=UUID once cross-workspace handling has passed it on', () => {
    const url = new URL('https://localhost:5174/#thread=abc-123');
    expect(stripDeepLinkFromUrl(url).hash).toBe('');
  });

  it('strips the JSON-encoded tap param after dispatch', () => {
    const tap = { kind: 'navigate', to: { target: 'thread', id: 't-9' } };
    const url = new URL(
      `https://localhost:5174/?other=keep#notification=n-1&tap=${encodeURIComponent(JSON.stringify(tap))}`,
    );
    const cleaned = stripDeepLinkFromUrl(url);
    expect(cleaned.search).toBe('?other=keep');
    expect(cleaned.hash).toBe('');
  });

  it('leaves the dropped legacy `app` key alone (no longer ours to manage)', () => {
    const url = new URL('https://localhost:5174/?app=stale');
    const cleaned = stripDeepLinkFromUrl(url);
    expect(cleaned.search).toBe('?app=stale');
  });
});


describe('validateTap refuses a scripted navigate url', () => {
  const tapWith = (url: string) =>
    new URL(
      `https://localhost:5174/#notification=n-1&tap=${encodeURIComponent(
        JSON.stringify({ kind: 'navigate', to: { target: 'url', url } }),
      )}`,
    );

  it('accepts an ordinary http url', () => {
    const parsed = parseDeepLinkFromUrl(tapWith('https://example.com/a'));
    expect(parsed.tap).toEqual({
      kind: 'navigate',
      to: { target: 'url', url: 'https://example.com/a' },
    });
  });

  it('accepts the non-http schemes openExternalUrl documents', () => {
    for (const url of ['mailto:me@example.com', 'tel:+1234', 'file:///tmp/x']) {
      expect(parseDeepLinkFromUrl(tapWith(url)).tap).not.toBeNull();
    }
  });

  it.each([
    ['bare', 'javascript:alert(1)'],
    ['upper case', 'JavaScript:alert(1)'],
    ['vbscript', 'vbscript:msgbox(1)'],
    ['leading space', ' javascript:alert(1)'],
    ['inner tab', 'java\tscript:alert(1)'],
    ['inner newline', 'java\nscript:alert(1)'],
  ])('refuses a %s scripted url', (_label, url) => {
    expect(parseDeepLinkFromUrl(tapWith(url)).tap).toBeNull();
  });

  it.each([[0x00], [0x01], [0x08], [0x0e], [0x1f]])(
    'refuses a scripted url hidden behind C0 control %#',
    (code) => {
      const url = `${String.fromCharCode(code)}javascript:alert(1)`;
      expect(new URL(url).protocol).toBe('javascript:');
      expect(parseDeepLinkFromUrl(tapWith(url)).tap).toBeNull();
    },
  );
});

describe('parseDeepLinkFromInboxRow', () => {
  const row = {
    id: 'n-1',
    thread_id: 't-1',
    event_id: 'e-1',
    tap: { kind: 'navigate', to: { target: 'thread', id: 't-1', event_id: 'e-1' } } as Tap,
  };

  it('carries the row id, its source thread and event, and its tap', () => {
    expect(parseDeepLinkFromInboxRow(row)).toEqual({
      notification: 'n-1',
      thread: 't-1',
      event: 'e-1',
      tap: row.tap,
    });
  });

  it('normalizes absent fields to null, as the URL and SW parsers do', () => {
    expect(parseDeepLinkFromInboxRow({ id: 'n-2' })).toEqual({
      notification: 'n-2',
      thread: null,
      event: null,
      tap: null,
    });
  });

  it('resolves a navigate row to its destination, so one tap lands there', () => {
    expect(resolveDeepLink(parseDeepLinkFromInboxRow(row))).toEqual({
      type: 'navigate',
      to: { target: 'thread', id: 't-1', event_id: 'e-1' },
      notification: 'n-1',
    });
  });

  it.each<[string, Tap | undefined]>([
    ['a modal tap', { kind: 'modal' }],
    ['no tap at all', undefined],
  ])('resolves %s to the detail, exactly as the whole list did before', (_label, tap) => {
    expect(resolveDeepLink(parseDeepLinkFromInboxRow({ id: 'n-3', tap }))).toEqual({
      type: 'view-notification',
      id: 'n-3',
    });
  });
});

describe('restore notification taps', () => {
  const tap = { kind: 'restore-snapshot' as const, workspace_id: 'demo', handle: 'snap_00000000000000000000000000' };
  it('preserves Undo through URL, push and inbox routes as confirmation intent', () => {
    const targets = [
      parseDeepLinkFromUrl(new URL('https://example.com/?notification=n&tap=' + encodeURIComponent(JSON.stringify(tap)))),
      parseDeepLinkFromSwMessage({ notification_id: 'n', tap })!,
      parseDeepLinkFromInboxRow({ id: 'n', tap }),
    ];
    for (const target of targets) {
      expect(resolveDeepLink(target)).toEqual({ type: 'restore-snapshot', tap, notification: 'n' });
    }
  });
  it.each(['../x', 'snap_80000000000000000000000000', 'snap_0000000000000000000000000I'])('rejects malformed handle %s', (handle) => {
    expect(parseDeepLinkFromSwMessage({ notification_id: 'n', tap: { ...tap, handle } })?.tap).toBeNull();
  });
});

describe('config undo notification taps', () => {
  const tap = { kind: 'undo-config-change' as const, event_id: '0b3f2c1e-8d4a-4c6e-9f10-2a3b4c5d6e7f' };
  it('preserves Undo through URL, push and inbox routes as confirmation intent', () => {
    const targets = [
      parseDeepLinkFromUrl(new URL('https://example.com/?notification=n&tap=' + encodeURIComponent(JSON.stringify(tap)))),
      parseDeepLinkFromSwMessage({ notification_id: 'n', tap })!,
      parseDeepLinkFromInboxRow({ id: 'n', tap }),
    ];
    for (const target of targets) {
      expect(resolveDeepLink(target)).toEqual({ type: 'undo-config-change', tap, notification: 'n' });
    }
  });
  it.each(['../x', '', 'not-a-uuid'])('rejects malformed event id %s', (event_id) => {
    expect(parseDeepLinkFromSwMessage({ notification_id: 'n', tap: { ...tap, event_id } })?.tap).toBeNull();
  });
});

describe('push source provenance', () => {
  it('preserves UUID, original and unknown provenance in both transports', () => {
    for (const source of ['11111111-1111-4111-8111-111111111111', 'none', 'unknown']) {
      const url = new URL('https://example.com/?notification=n&restore_generation=' + source);
      expect(parseDeepLinkFromUrl(url).restoreGeneration).toBe(source);
      expect(parseDeepLinkFromSwMessage({ notification_id: 'n',
        restore_generation: source === 'none' ? null : source })?.restoreGeneration).toBe(source);
      expect(stripDeepLinkFromUrl(url).searchParams.has('restore_generation')).toBe(false);
    }
    expect(parseDeepLinkFromUrl(new URL('https://example.com/?notification=n')).restoreGeneration).toBe('unknown');
    expect(parseDeepLinkFromSwMessage({ notification_id: 'n' })?.restoreGeneration).toBe('unknown');
  });
});
