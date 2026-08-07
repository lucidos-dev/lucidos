/**
 * Which SHAPE the app popout control takes, per platform.
 *
 * The shape is the entire fix, and it is invisible in the rendered markup: a
 * `<a target="_blank">` that opens a tab and one that silently opens nothing are
 * the same three attributes. Only the packaged desktop client is in the second
 * case (WKWebView drops a `_blank` navigation unless wry was given a new-window
 * delegate, which only the in-app browser preview webview has), so only there
 * does the control become a button that routes through the OS opener.
 *
 * Pinned as the action SPEC rather than through a mounted header: the header
 * also runs progressive collapse, which decides whether this action renders as
 * a header button or as an overflow-menu row, and both renderings read the same
 * spec. Asserting the spec covers both and cannot be fooled by which one the
 * measured width happened to pick.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';

const platform = vi.hoisted(() => ({ isTauri: false, isIOSPwa: false }));
vi.mock('../../../utils/platform', () => ({
  isTauri: () => platform.isTauri,
  isIOS: () => false,
  isIOSPwa: () => platform.isIOSPwa,
}));

const popOutApp = vi.hoisted(() => vi.fn());
vi.mock('../../../store/actions/apps', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/apps')>()),
  getAppFrameSrc: () => '/dev/app/habit-tracker/',
  popOutApp,
}));

const { appPopoutAction } = await import('../ContentHeaderActions');

describe('appPopoutAction', () => {
  beforeEach(() => {
    platform.isTauri = false;
    platform.isIOSPwa = false;
    popOutApp.mockClear();
  });

  it('browser: a real anchor, so cmd-click and "copy link address" keep working', () => {
    const action = appPopoutAction();

    expect(action?.href).toBe('/dev/app/habit-tracker/');
    // `href !== undefined` is what makes both renderers emit an `<a>`, so an
    // onClick here would silently demote the control to a button.
    expect(action?.onClick).toBeUndefined();
    expect(action?.label).toBe('Open in new tab');
  });

  it('packaged desktop client: a button that routes through the OS opener', () => {
    platform.isTauri = true;

    const action = appPopoutAction();

    // No href at all, not a null one: the renderers branch on `!== undefined`,
    // so `href: null` would still emit the anchor that does nothing here.
    expect(action?.href).toBeUndefined();
    // The tests run in vitest's node environment (see src/test-setup.ts), which
    // has no MouseEvent; the handler ignores its argument anyway.
    action?.onClick?.({} as MouseEvent);
    expect(popOutApp).toHaveBeenCalledTimes(1);
  });

  it('packaged desktop client: says "browser", because there are no tabs there', () => {
    platform.isTauri = true;
    // The label is also the aria-label, the tooltip and the overflow-menu row
    // text, so "new tab" would be wrong in three places at once.
    expect(appPopoutAction()?.label).toBe('Open in browser');
  });

  it('installed iOS PWA: no control at all', () => {
    platform.isIOSPwa = true;
    expect(appPopoutAction()).toBeNull();
  });

  it('keeps the key and the addressing class on every platform', () => {
    for (const tauri of [false, true]) {
      platform.isTauri = tauri;
      const action = appPopoutAction();
      // `extraClass` is how the rest of the app and the e2e suite address this
      // action wherever collapse put it; `key` is what the collapse hook counts.
      expect(action?.key).toBe('open-in-tab');
      expect(action?.extraClass).toBe('app-open-in-tab');
    }
  });
});
