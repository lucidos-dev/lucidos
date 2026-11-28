import { beforeEach, expect, it, vi } from 'vitest';
const m = vi.hoisted(() => ({ warm: false, preload: vi.fn(), navigate: vi.fn(), url: vi.fn(), compose: vi.fn(), toast: vi.fn() }));
vi.mock('./navigationDirect', () => ({ navigateToUrl: m.url, navigateToNewChat: m.compose }));
vi.mock('../../utils/lazyComponent', () => ({ lazyModule: () => ({ isLoaded: () => m.warm, preload: m.preload, get: () => ({ handleNavigationRequest: m.navigate }) }) }));
vi.mock('../../utils/idlePrefetch', () => ({ prefetchWhenIdle: vi.fn() }));
vi.mock('../store', () => ({ showToast: m.toast }));
import { handleNavigationRequest } from './navigation-dispatch';
beforeEach(() => { vi.clearAllMocks(); m.warm = false; m.preload.mockReset().mockResolvedValue(true); });

it('preserves browser activation for URL opening and new-chat focus while cold', () => {
  const url = { target: 'url', url: 'https://example.com' };
  const compose = { target: 'new-chat', prompt: 'A draft' };
  handleNavigationRequest(url, { source: 'a thread' });
  handleNavigationRequest(compose);
  expect(m.url).toHaveBeenCalledWith(url, 'a thread');
  expect(m.compose).toHaveBeenCalledWith(compose, undefined);
  expect(m.preload).not.toHaveBeenCalled();
});

it('retries the same destination and source after a chunk failure', async () => {
  m.preload.mockResolvedValueOnce(false);
  const nav = { target: 'file', file_path: 'artifacts/example.md' };
  const options = { source: 'a thread', find: 'example' };
  await handleNavigationRequest(nav, options);
  expect(m.navigate).not.toHaveBeenCalled();
  m.toast.mock.calls[0][2].action.onClick();
  await vi.waitFor(() => expect(m.navigate).toHaveBeenCalledWith(nav, options));
});

it('forwards a warm destination synchronously', () => {
  m.warm = true;
  handleNavigationRequest({ target: 'files' });
  expect(m.navigate).toHaveBeenCalledOnce();
  expect(m.preload).not.toHaveBeenCalled();
});
