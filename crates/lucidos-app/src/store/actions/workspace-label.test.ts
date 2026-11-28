
import { describe, it, expect, beforeEach, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  listWorkspaces: vi.fn(),
  getWorkspaceLabel: vi.fn(),
  workspaceId: null as string | null,
  isPicker: false,
}));

vi.mock('../../api/client/control', () => ({ listWorkspaces: mocks.listWorkspaces }));
vi.mock('../../api/client/chat', async () => {
  const actual = await vi.importActual<typeof import('../../api/client/chat')>('../../api/client/chat');
  return { ...actual, getWorkspaceLabel: mocks.getWorkspaceLabel };
});
vi.mock('../../utils/basePath', async () => {
  const actual = await vi.importActual<typeof import('../../utils/basePath')>('../../utils/basePath');
  return {
    ...actual,
    get WORKSPACE_ID() {
      return mocks.workspaceId;
    },
    get IS_PICKER() {
      return mocks.isPicker;
    },
  };
});

const { adoptWorkspaceDisplayName, loadWorkspaceDisplayName } = await import('./workspace-label');
const { workspaceName, workspaceDisplayName, visibleWorkspaceName } = await import('../store');

const entry = (id: string, name: string) => ({
  id,
  name,
  port: 5000,
  health: 'healthy' as const,
  autostart: true,
});

beforeEach(() => {
  vi.clearAllMocks();
  mocks.workspaceId = 'personal';
  mocks.isPicker = false;
  mocks.getWorkspaceLabel.mockResolvedValue(null);
  workspaceName.value = 'personal';
  workspaceDisplayName.value = '';
});

describe('behind the gateway, the label comes from the control listing', () => {
  it('adopts the registry label for this slug', async () => {
    mocks.listWorkspaces.mockResolvedValue([entry('other', 'Other'), entry('personal', 'personaal')]);
    await loadWorkspaceDisplayName();
    expect(visibleWorkspaceName.value).toBe('personaal');
  });

  it('falls back to the engine name until the label lands', () => {
    expect(visibleWorkspaceName.value).toBe('personal');
  });

  it('keeps the engine name when the gateway does not list us', async () => {
    mocks.listWorkspaces.mockResolvedValue([entry('other', 'Other')]);
    await loadWorkspaceDisplayName();
    expect(visibleWorkspaceName.value).toBe('personal');
  });

  it('never detours through the engine, which has nothing to add here', async () => {
    mocks.listWorkspaces.mockResolvedValue([entry('personal', 'personaal')]);
    await loadWorkspaceDisplayName();
    expect(mocks.getWorkspaceLabel).not.toHaveBeenCalled();
  });

  it('survives an unreachable control plane without breaking the display', async () => {
    mocks.listWorkspaces.mockRejectedValue(new Error('gateway control unreachable'));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await expect(loadWorkspaceDisplayName()).resolves.toBeUndefined();
    expect(visibleWorkspaceName.value).toBe('personal');
    warn.mockRestore();
  });

  it('picks our own row out of a listing full of other workspaces', async () => {
    mocks.listWorkspaces.mockResolvedValue([
      entry('other', 'Other'),
      entry('personal', 'renamed again'),
      entry('third', 'Third'),
    ]);
    await loadWorkspaceDisplayName();
    expect(visibleWorkspaceName.value).toBe('renamed again');
  });

  it('re-adopts from a listing someone else fetched, so a rename lands without a reload', () => {
    adoptWorkspaceDisplayName([entry('personal', 'renamed again')]);
    expect(visibleWorkspaceName.value).toBe('renamed again');
  });

  it('ignores a listing that does not mention us', () => {
    adoptWorkspaceDisplayName([entry('other', 'Other')]);
    expect(visibleWorkspaceName.value).toBe('personal');
  });
});

describe('on the engine port, the label comes from the engine', () => {
  beforeEach(() => {
    mocks.workspaceId = null;
    workspaceName.value = 'dev';
  });

  it('shows the renamed workspace, not the engine directory name', async () => {
    mocks.getWorkspaceLabel.mockResolvedValue('development');
    await loadWorkspaceDisplayName();
    expect(visibleWorkspaceName.value).toBe('development');
  });

  it('never reaches for the control listing, which is on another origin', async () => {
    mocks.getWorkspaceLabel.mockResolvedValue('development');
    await loadWorkspaceDisplayName();
    expect(mocks.listWorkspaces).not.toHaveBeenCalled();
  });

  it('keeps the engine name when the engine has no gateway to ask', async () => {
    mocks.getWorkspaceLabel.mockResolvedValue(null);
    await loadWorkspaceDisplayName();
    expect(visibleWorkspaceName.value).toBe('dev');
  });

  it('keeps the engine name when the route is missing or unreachable', async () => {
    mocks.getWorkspaceLabel.mockRejectedValue(new Error('404'));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await expect(loadWorkspaceDisplayName()).resolves.toBeUndefined();
    expect(visibleWorkspaceName.value).toBe('dev');
    warn.mockRestore();
  });

  it('adopts nothing from a listing, having no slug to match on', () => {
    adoptWorkspaceDisplayName([entry('dev', 'development')]);
    expect(visibleWorkspaceName.value).toBe('dev');
  });
});

describe('the picker asks nobody', () => {
  it('has no single workspace to label', async () => {
    mocks.workspaceId = null;
    mocks.isPicker = true;
    await loadWorkspaceDisplayName();
    expect(mocks.listWorkspaces).not.toHaveBeenCalled();
    expect(mocks.getWorkspaceLabel).not.toHaveBeenCalled();
  });
});

describe('identity is not the label', () => {
  it('leaves the engine-reported name alone', async () => {
    mocks.listWorkspaces.mockResolvedValue([entry('personal', 'personaal')]);
    await loadWorkspaceDisplayName();
    expect(workspaceName.value).toBe('personal');
  });
});

it('does not adopt the local namesake when served under a remote route', () => {
  mocks.workspaceId = 'dev@cloud';
  workspaceName.value = 'dev';
  adoptWorkspaceDisplayName([entry('dev', 'Local dev')]);
  expect(visibleWorkspaceName.value).toBe('dev');
});
