import { describe, it, expect, vi } from 'vitest';
import { buildAppLocalUrl, data } from './data';

const BASE = 'https://host.example';

it.each([
  ['PUT', () => data.write('artifacts/notes.txt', 'new text')],
  ['DELETE', () => data.delete('artifacts/notes.txt')],
] as const)('gives %s time to receive the repo busy refusal', async (method, mutate) => {
  const deadline = vi.spyOn(AbortSignal, 'timeout');
  const fetch = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response(
    JSON.stringify({ error: 'Workspace repository is busy. Nothing was written.' }),
    { status: 503, headers: { 'Content-Type': 'application/json' } },
  ));
  try {
    await expect(mutate()).rejects.toMatchObject({
      httpCode: 503,
      reason: 'Workspace repository is busy. Nothing was written.',
    });
    expect(deadline).toHaveBeenCalledWith(60_000);
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(fetch).toHaveBeenCalledWith(expect.any(String), expect.objectContaining({ method }));
  } finally {
    deadline.mockRestore();
    fetch.mockRestore();
  }
});

describe('buildAppLocalUrl', () => {
  describe('rewrites own-app assets to /app/<id>/…', () => {
    it('preserves thread_id from iframe URL (WIP-preview)', () => {
      expect(
        buildAppLocalUrl(
          'apps/habit-tracker/icon.png',
          '/app/habit-tracker/',
          '?thread_id=abc-123',
          BASE,
        ),
      ).toBe(`${BASE}/app/habit-tracker/icon.png?thread_id=abc-123`);
    });

    it('emits no query when iframe is live (no thread_id)', () => {
      expect(
        buildAppLocalUrl(
          'apps/habit-tracker/icon.png',
          '/app/habit-tracker/',
          '',
          BASE,
        ),
      ).toBe(`${BASE}/app/habit-tracker/icon.png`);
    });

    it('encodes path segments but preserves the slashes', () => {
      expect(
        buildAppLocalUrl(
          'apps/habit-tracker/sub dir/file name.png',
          '/app/habit-tracker/',
          '',
          BASE,
        ),
      ).toBe(`${BASE}/app/habit-tracker/sub%20dir/file%20name.png`);
    });

    it('matches when iframe path has subroutes', () => {
      expect(
        buildAppLocalUrl(
          'apps/habit-tracker/icon.png',
          '/app/habit-tracker/index.html',
          '?thread_id=abc',
          BASE,
        ),
      ).toBe(`${BASE}/app/habit-tracker/icon.png?thread_id=abc`);
    });

    it('matches when iframe path has nested subroutes', () => {
      expect(
        buildAppLocalUrl(
          'apps/habit-tracker/icon.png',
          '/app/habit-tracker/foo/bar',
          '',
          BASE,
        ),
      ).toBe(`${BASE}/app/habit-tracker/icon.png`);
    });

    it('matches behind the gateway, where the path carries a workspace slug', () => {
      expect(
        buildAppLocalUrl(
          'apps/habit-tracker/icon.png',
          '/dev/app/habit-tracker/',
          '?thread_id=abc-123',
          '/dev',
        ),
      ).toBe('/dev/app/habit-tracker/icon.png?thread_id=abc-123');
    });
  });

  describe('returns null (caller falls through to /data/…)', () => {
    it('cross-app reference — never serves another app from this worktree', () => {
      expect(
        buildAppLocalUrl(
          'apps/other-app/icon.png',
          '/app/habit-tracker/',
          '?thread_id=abc',
          BASE,
        ),
      ).toBeNull();
    });

    it('non-apps path (artifacts)', () => {
      expect(
        buildAppLocalUrl(
          'artifacts/screenshots/latest.png',
          '/app/habit-tracker/',
          '?thread_id=abc',
          BASE,
        ),
      ).toBeNull();
    });

    it('non-apps path (knowhow)', () => {
      expect(
        buildAppLocalUrl(
          'knowhow/foo.md',
          '/app/habit-tracker/',
          '',
          BASE,
        ),
      ).toBeNull();
    });

    it('SDK loaded outside an app iframe', () => {
      expect(
        buildAppLocalUrl(
          'apps/habit-tracker/icon.png',
          '/',
          '',
          BASE,
        ),
      ).toBeNull();
    });

    it('app folder reference without a sub-path (no file to serve)', () => {
      expect(
        buildAppLocalUrl(
          'apps/habit-tracker/',
          '/app/habit-tracker/',
          '',
          BASE,
        ),
      ).toBeNull();
    });

    it('app folder reference without trailing slash', () => {
      expect(
        buildAppLocalUrl(
          'apps/habit-tracker',
          '/app/habit-tracker/',
          '',
          BASE,
        ),
      ).toBeNull();
    });

    it('legacy /api/app/ prefix — route has moved, do not rewrite', () => {
      expect(
        buildAppLocalUrl(
          'apps/habit-tracker/icon.png',
          '/api/app/habit-tracker/',
          '',
          BASE,
        ),
      ).toBeNull();
    });
  });

  it('drops unrelated query params (only carries thread_id)', () => {
    expect(
      buildAppLocalUrl(
        'apps/habit-tracker/icon.png',
        '/app/habit-tracker/',
        '?thread_id=abc&unrelated=x&debug=1',
        BASE,
      ),
    ).toBe(`${BASE}/app/habit-tracker/icon.png?thread_id=abc`);
  });

  describe('the frame capability (ADR 0238)', () => {
    const CARRIER = '/~cap/6a0b~habit-tracker~00ff';

    it('sits between the workspace prefix and /app/, which is where the gateway reads it', () => {
      expect(
        buildAppLocalUrl(
          'apps/habit-tracker/icon.png',
          '/dev/app/habit-tracker/',
          '',
          '/dev',
          CARRIER,
        ),
      ).toBe(`/dev${CARRIER}/app/habit-tracker/icon.png`);
    });

    it('rides beside the WIP-preview thread_id rather than instead of it', () => {
      expect(
        buildAppLocalUrl(
          'apps/habit-tracker/icon.png',
          '/dev/app/habit-tracker/',
          '?thread_id=abc',
          '/dev',
          CARRIER,
        ),
      ).toBe(`/dev${CARRIER}/app/habit-tracker/icon.png?thread_id=abc`);
    });

    it('is absent direct to an engine, where there is no gate to pass', () => {
      expect(
        buildAppLocalUrl('apps/habit-tracker/icon.png', '/app/habit-tracker/', '', BASE, ''),
      ).toBe(`${BASE}/app/habit-tracker/icon.png`);
    });
  });
});
