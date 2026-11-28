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
  appPopoutHref: () => '/dev/app/habit-tracker/',
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
    expect(action?.onClick).toBeUndefined();
    expect(action?.label).toBe('Open in new tab');
  });

  it('packaged desktop client: a button that routes through the OS opener', () => {
    platform.isTauri = true;

    const action = appPopoutAction();

    expect(action?.href).toBeUndefined();
    action?.onClick?.({} as MouseEvent);
    expect(popOutApp).toHaveBeenCalledTimes(1);
  });

  it('packaged desktop client: says "browser", because there are no tabs there', () => {
    platform.isTauri = true;
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
      expect(action?.key).toBe('open-in-tab');
      expect(action?.extraClass).toBe('app-open-in-tab');
    }
  });
});
