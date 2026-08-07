/**
 * Which name a workspace shows to the person inside it.
 *
 * The bug: renaming in the picker writes the gateway registry and tells the
 * engine nothing, so the app kept showing the engine's directory name while the
 * in-app switcher beside it showed the new label.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  listWorkspaces: vi.fn(),
  // Stand-in for basePath's load-time `WORKSPACE_ID` const: non-null = served
  // behind the gateway under `/<slug>/`, null = straight at an engine port.
  workspaceId: null as string | null,
}));

vi.mock('../../api/client/control', () => ({ listWorkspaces: mocks.listWorkspaces }));
// Keep the rest of basePath real: the store pulls in the API client, which
// reads BASE_PATH at import time.
vi.mock('../../utils/basePath', async () => {
  const actual = await vi.importActual<typeof import('../../utils/basePath')>('../../utils/basePath');
  return {
    ...actual,
    get WORKSPACE_ID() {
      return mocks.workspaceId;
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
  workspaceName.value = 'personal';
  workspaceDisplayName.value = '';
});

describe('the workspace shows the name the user gave it', () => {
  it('adopts the registry label for this slug', async () => {
    // Created as "personal", renamed to "personaal": the address stays.
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

  it('does not ask when there is no gateway in front of us', async () => {
    mocks.workspaceId = null;
    await loadWorkspaceDisplayName();
    expect(mocks.listWorkspaces).not.toHaveBeenCalled();
    expect(visibleWorkspaceName.value).toBe('personal');
  });

  it('survives an unreachable control plane without breaking the display', async () => {
    // Legacy no-gateway mode has no such route; the engine name is the answer.
    mocks.listWorkspaces.mockRejectedValue(new Error('no gateway control here'));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await expect(loadWorkspaceDisplayName()).resolves.toBeUndefined();
    expect(visibleWorkspaceName.value).toBe('personal');
    warn.mockRestore();
  });

  it('re-adopts from a listing someone else fetched, so a rename lands without a reload', () => {
    // The switcher refetches on every open; that listing updates the header too.
    adoptWorkspaceDisplayName([entry('personal', 'renamed again')]);
    expect(visibleWorkspaceName.value).toBe('renamed again');
  });
});

describe('identity is not the label', () => {
  it('leaves the engine-reported name alone', async () => {
    mocks.listWorkspaces.mockResolvedValue([entry('personal', 'personaal')]);
    await loadWorkspaceDisplayName();
    // Thread-ref links embed this in durable text and cross-workspace routing
    // matches on it, so a rename must not move it.
    expect(workspaceName.value).toBe('personal');
  });
});
