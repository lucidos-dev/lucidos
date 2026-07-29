import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  isTauri: vi.fn(() => true),
  checkAppUpdate: vi.fn(),
  installAppUpdateAndRestart: vi.fn(),
  showToast: vi.fn(),
}));

vi.mock('../../utils/platform', () => ({ isTauri: mocks.isTauri }));
vi.mock('../../utils/tauri', () => ({
  checkAppUpdate: mocks.checkAppUpdate,
  installAppUpdateAndRestart: mocks.installAppUpdateAndRestart,
}));
vi.mock('../store', () => ({ showToast: mocks.showToast }));

const { checkForAppUpdate } = await import('./app-update');

beforeEach(() => {
  mocks.isTauri.mockReturnValue(true);
  mocks.checkAppUpdate.mockReset();
  mocks.installAppUpdateAndRestart.mockReset();
  mocks.showToast.mockReset();
});

afterEach(() => vi.restoreAllMocks());

describe('checkForAppUpdate', () => {
  it('is a no-op outside the Tauri client (browser / PWA / dev)', async () => {
    mocks.isTauri.mockReturnValue(false);
    await checkForAppUpdate();
    expect(mocks.checkAppUpdate).not.toHaveBeenCalled();
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  it('shows no toast when there is no update', async () => {
    mocks.checkAppUpdate.mockResolvedValue(null);
    await checkForAppUpdate();
    expect(mocks.checkAppUpdate).toHaveBeenCalledTimes(1);
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  it('surfaces the in-app "Update & restart" toast when an update is available', async () => {
    mocks.checkAppUpdate.mockResolvedValue('2026.6.25');
    await checkForAppUpdate();
    expect(mocks.showToast).toHaveBeenCalledTimes(1);
    const [message, type, opts] = mocks.showToast.mock.calls[0];
    expect(message).toContain('2026.6.25');
    expect(type).toBe('info');
    expect(opts.key).toBe('app-update-available');
    expect(opts.action.label).toBe('Update & restart');
  });

  it('clicking the toast action installs the update + restarts the stack', async () => {
    mocks.checkAppUpdate.mockResolvedValue('2026.6.25');
    mocks.installAppUpdateAndRestart.mockResolvedValue(undefined);
    await checkForAppUpdate();
    const opts = mocks.showToast.mock.calls[0][2];
    opts.action.onClick();
    expect(mocks.installAppUpdateAndRestart).toHaveBeenCalledTimes(1);
  });

  it('swallows a failed check (best-effort) — no toast, retried next poll', async () => {
    mocks.checkAppUpdate.mockRejectedValue(new Error('network'));
    await checkForAppUpdate();
    expect(mocks.showToast).not.toHaveBeenCalled();
  });
});
