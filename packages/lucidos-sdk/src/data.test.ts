import { describe, it, expect } from 'vitest';
import { buildAppLocalUrl } from './data';

const BASE = 'https://host.example';

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
});
