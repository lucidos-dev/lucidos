import { describe, it, expect, beforeEach, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  fetchWorkspaces: vi.fn(),
  openUrl: vi.fn(),
  showToast: vi.fn(),
  isTauri: vi.fn(() => false),
  windowOpen: vi.fn(),
}));

vi.mock('../../api/client', () => ({ fetchWorkspaces: mocks.fetchWorkspaces }));
vi.mock('./artifacts', () => ({ openUrl: mocks.openUrl }));
vi.mock('../../utils/platform', () => ({ isTauri: mocks.isTauri }));

vi.mock('../store', async () => {
  const actual = await vi.importActual<typeof import('../store')>('../store');
  return { ...actual, showToast: mocks.showToast };
});

const { openThreadInWorkspace } = await import('./cross-workspace');

const TID = '1c2419a1-aaaa-bbbb-cccc-ddddeeeeffff';

const wsInfo = (overrides: Partial<{ name: string; port: number | null; engine_running: boolean }>) => ({
  name: 'work',
  path: '/tmp/x',
  port: 5175 as number | null,
  engine_running: true,
  engine_version: 'test',
  ...overrides,
});

describe('openThreadInWorkspace', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.isTauri.mockReturnValue(false);
    vi.stubGlobal('window', { ...window, open: mocks.windowOpen });
  });

  it('opens a named browser tab for the target workspace when running', async () => {
    mocks.fetchWorkspaces.mockResolvedValue({
      workspaces: [wsInfo({ name: 'work', port: 5175 }), wsInfo({ name: 'personal', port: 5174 })],
    });

    await openThreadInWorkspace('work', TID);

    expect(mocks.windowOpen).toHaveBeenCalledWith(`https://localhost:5175/#thread=${TID}`, 'lucidos-ws-work');
    expect(mocks.openUrl).not.toHaveBeenCalled();
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  it('uses openUrl (panel) under Tauri', async () => {
    mocks.isTauri.mockReturnValue(true);
    mocks.fetchWorkspaces.mockResolvedValue({ workspaces: [wsInfo({ name: 'work', port: 5175 })] });

    await openThreadInWorkspace('work', TID);

    expect(mocks.openUrl).toHaveBeenCalledWith(`https://localhost:5175/#thread=${TID}`);
    expect(mocks.windowOpen).not.toHaveBeenCalled();
  });

  it('shows an error toast when the workspace is not in the list', async () => {
    mocks.fetchWorkspaces.mockResolvedValue({ workspaces: [wsInfo({ name: 'personal' })] });

    await openThreadInWorkspace('work', TID);

    expect(mocks.windowOpen).not.toHaveBeenCalled();
    expect(mocks.openUrl).not.toHaveBeenCalled();
    expect(mocks.showToast).toHaveBeenCalledWith(
      expect.stringContaining("Workspace 'work' not found"),
      'error',
    );
  });

  it('shows an error toast when the workspace exists but engine is not running', async () => {
    mocks.fetchWorkspaces.mockResolvedValue({ workspaces: [wsInfo({ name: 'work', engine_running: false })] });

    await openThreadInWorkspace('work', TID);

    expect(mocks.windowOpen).not.toHaveBeenCalled();
    expect(mocks.showToast).toHaveBeenCalledWith(
      expect.stringContaining("Workspace 'work' is not running"),
      'error',
    );
  });

  it('shows an error toast when the workspace has no port assigned', async () => {
    mocks.fetchWorkspaces.mockResolvedValue({ workspaces: [wsInfo({ name: 'work', port: null })] });

    await openThreadInWorkspace('work', TID);

    expect(mocks.windowOpen).not.toHaveBeenCalled();
    expect(mocks.showToast).toHaveBeenCalledWith(
      expect.stringContaining("Workspace 'work' is not running"),
      'error',
    );
  });

  it('shows an error toast when the API request fails', async () => {
    mocks.fetchWorkspaces.mockRejectedValue(new Error('boom'));

    await openThreadInWorkspace('work', TID);

    expect(mocks.windowOpen).not.toHaveBeenCalled();
    expect(mocks.showToast).toHaveBeenCalledWith(
      expect.stringContaining('Failed to open thread'),
      'error',
    );
  });
});
