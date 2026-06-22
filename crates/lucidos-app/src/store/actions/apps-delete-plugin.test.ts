import { describe, it, expect, beforeEach, vi } from 'vitest';
import { signal } from '@preact/signals-core';

// Real ApiError shape (httpCode + parsed body) so `instanceof` + `.body`
// resolve exactly as the production class does — confirmDeleteApp keys its
// redirect on both.
class ApiError extends Error {
  constructor(
    public readonly httpCode: number,
    public readonly reason: string,
    public readonly body?: unknown,
  ) {
    super(`${httpCode} ${reason}`);
    this.name = 'ApiError';
  }
}

const showToast = vi.fn();
const showConfirm = vi.fn().mockResolvedValue(true);

vi.mock('../store', () => ({
  appsList: signal({ status: 'loaded', data: [] }),
  currentApp: signal(null),
  panelOverlay: signal(null),
  closeInlineForm: vi.fn(),
  pendingChatMessage: signal(null),
  showToast,
  showConfirm,
  appPseudoFullscreen: signal(false),
  appRefreshKey: signal(0),
  wipPreviewThreadId: signal(null),
  threadMap: signal(new Map()),
  appsTab: signal('installed'),
  appSearchOpen: signal(false),
  appSearchQuery: signal(''),
}));

const deleteAppApi = vi.fn();
const stagePluginUninstall = vi.fn();
const listAppsApi = vi.fn().mockResolvedValue([]);

vi.mock('../../api/client', () => ({
  ApiError,
  deleteAppApi: (...a: unknown[]) => deleteAppApi(...a),
  stagePluginUninstall: (...a: unknown[]) => stagePluginUninstall(...a),
  listAppsApi: (...a: unknown[]) => listAppsApi(...a),
  updateAppApi: vi.fn(),
  appUrl: vi.fn((id: string) => `/app/${id}/`),
  postAppCapture: vi.fn(),
}));

const openPluginUninstallRequest = vi.fn();
vi.mock('./plugin-uninstall', () => ({
  openPluginUninstallRequest: (...a: unknown[]) => openPluginUninstallRequest(...a),
}));
vi.mock('./navigation', () => ({ pushNavState: vi.fn() }));
vi.mock('./pane', () => ({ revealContentPane: vi.fn() }));
vi.mock('./wipPreview', () => ({ clearWipIfMatches: vi.fn() }));
vi.mock('../../components/chat/scrollState', () => ({ isElementVisible: vi.fn(() => true) }));

describe('confirmDeleteApp — plugin ownership', () => {
  beforeEach(() => {
    deleteAppApi.mockReset();
    stagePluginUninstall.mockReset();
    openPluginUninstallRequest.mockReset();
    showToast.mockClear();
    showConfirm.mockClear().mockResolvedValue(true);
  });

  it('redirects to the plugin uninstall panel on a 409 (plugin-owned app)', async () => {
    // Engine refuses the raw delete with the owning plugin id/name.
    deleteAppApi.mockRejectedValue(
      new ApiError(409, 'owned by plugin', {
        plugin_id: 'no-role-playing',
        plugin_name: 'No role playing',
      }),
    );
    const staged = { uninstall_id: 'u1', plugin_id: 'no-role-playing', files: [] };
    stagePluginUninstall.mockResolvedValue(staged);

    const { confirmDeleteApp } = await import('./apps');
    await confirmDeleteApp('anti-sycophancy-critique', 'No role playing');

    // Stages the uninstall for the OWNING PLUGIN, not the app id.
    expect(stagePluginUninstall).toHaveBeenCalledWith('no-role-playing');
    expect(openPluginUninstallRequest).toHaveBeenCalledWith(staged);
    // No dead-end error toast — the user was routed, not failed.
    expect(showToast).not.toHaveBeenCalled();
  });

  it('does NOT redirect for a standalone app delete (success)', async () => {
    deleteAppApi.mockResolvedValue({ commit: 'abc123' });

    const { confirmDeleteApp } = await import('./apps');
    await confirmDeleteApp('standalone-app', 'Standalone App');

    expect(deleteAppApi).toHaveBeenCalledWith('standalone-app');
    expect(stagePluginUninstall).not.toHaveBeenCalled();
    expect(openPluginUninstallRequest).not.toHaveBeenCalled();
    expect(showToast).not.toHaveBeenCalled();
  });

  it('shows an error toast for a non-409 failure (no redirect)', async () => {
    deleteAppApi.mockRejectedValue(new ApiError(500, 'boom'));

    const { confirmDeleteApp } = await import('./apps');
    await confirmDeleteApp('standalone-app', 'Standalone App');

    expect(openPluginUninstallRequest).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast.mock.calls[0][1]).toBe('error');
  });

  it('falls back to an error toast if staging the uninstall fails', async () => {
    deleteAppApi.mockRejectedValue(
      new ApiError(409, 'owned by plugin', { plugin_id: 'p', plugin_name: 'P' }),
    );
    stagePluginUninstall.mockRejectedValue(new Error('stage failed'));

    const { confirmDeleteApp } = await import('./apps');
    await confirmDeleteApp('p-app', 'P App');

    expect(openPluginUninstallRequest).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast.mock.calls[0][1]).toBe('error');
  });

  it('does nothing when the user cancels the confirm dialog', async () => {
    showConfirm.mockResolvedValue(false);

    const { confirmDeleteApp } = await import('./apps');
    await confirmDeleteApp('any-app', 'Any App');

    expect(deleteAppApi).not.toHaveBeenCalled();
    expect(stagePluginUninstall).not.toHaveBeenCalled();
  });
});
