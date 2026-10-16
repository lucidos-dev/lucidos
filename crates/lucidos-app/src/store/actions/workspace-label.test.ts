/**
 * Which name a workspace shows to the person inside it.
 *
 * The bug: renaming in the picker writes the gateway registry and tells the
 * engine nothing, so the app kept showing the engine's directory name while the
 * in-app switcher beside it showed the new label.
 *
 * Then the same bug again, on the OTHER way in: a page served on the engine's
 * own port has no slug and cannot read the gateway listing across origins, so it
 * returned early and showed the directory name forever. An installed iOS PWA sat
 * on that path. It asks its own engine now, so both routes end at the registry.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  listWorkspaces: vi.fn(),
  getWorkspaceLabel: vi.fn(),
  // Stand-ins for basePath's load-time consts: `workspaceId` non-null = served
  // behind the gateway under `/<slug>/`, null = the picker or an engine port.
  workspaceId: null as string | null,
  isPicker: false,
}));

vi.mock('../../api/client/control', () => ({ listWorkspaces: mocks.listWorkspaces }));
// Keep the rest of the engine client real: it is the barrel the store pulls in.
vi.mock('../../api/client/chat', async () => {
  const actual = await vi.importActual<typeof import('../../api/client/chat')>('../../api/client/chat');
  return { ...actual, getWorkspaceLabel: mocks.getWorkspaceLabel };
});
// Keep the rest of basePath real: the store pulls in the API client, which
// reads BASE_PATH at import time.
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

  it('never detours through the engine, which has nothing to add here', async () => {
    mocks.listWorkspaces.mockResolvedValue([entry('personal', 'personaal')]);
    await loadWorkspaceDisplayName();
    expect(mocks.getWorkspaceLabel).not.toHaveBeenCalled();
  });

  it('survives an unreachable control plane without breaking the display', async () => {
    // We are only on this branch because a gateway put us behind a slug, so the
    // failure is one that has stopped answering. The engine name is the answer.
    mocks.listWorkspaces.mockRejectedValue(new Error('gateway control unreachable'));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await expect(loadWorkspaceDisplayName()).resolves.toBeUndefined();
    expect(visibleWorkspaceName.value).toBe('personal');
    warn.mockRestore();
  });

  it('picks our own row out of a listing full of other workspaces', async () => {
    // Matching is on the slug, never on position: the listing is every
    // workspace on the machine, and ours is not first in it.
    mocks.listWorkspaces.mockResolvedValue([
      entry('other', 'Other'),
      entry('personal', 'renamed again'),
      entry('third', 'Third'),
    ]);
    await loadWorkspaceDisplayName();
    expect(visibleWorkspaceName.value).toBe('renamed again');
  });

  it('re-adopts from a listing someone else fetched, so a rename lands without a reload', () => {
    // The in-app switcher refetches every time its row is unfolded and hands
    // the listing here, which is what keeps the pill above the rows from
    // showing a pre-rename name while the rows show the new one.
    adoptWorkspaceDisplayName([entry('personal', 'renamed again')]);
    expect(visibleWorkspaceName.value).toBe('renamed again');
  });

  it('ignores a listing that does not mention us', () => {
    // The switcher hands over whatever the gateway returned, so the adopter has
    // to tolerate our row being absent rather than blanking the name.
    adoptWorkspaceDisplayName([entry('other', 'Other')]);
    expect(visibleWorkspaceName.value).toBe('personal');
  });
});

describe('on the engine port, the label comes from the engine', () => {
  beforeEach(() => {
    // No `<base href>` at all, which is what a direct engine port serves.
    mocks.workspaceId = null;
    workspaceName.value = 'dev';
  });

  it('shows the renamed workspace, not the engine directory name', async () => {
    // The reported case: the directory is still `dev`, the registry says
    // `development`, and an iOS PWA installed on the engine port said `dev`.
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
    // An engine older than `/api/v1/workspace-label` 404s it, which throws.
    mocks.getWorkspaceLabel.mockRejectedValue(new Error('404'));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await expect(loadWorkspaceDisplayName()).resolves.toBeUndefined();
    expect(visibleWorkspaceName.value).toBe('dev');
    warn.mockRestore();
  });

  it('adopts nothing from a listing, having no slug to match on', () => {
    // The switcher gates itself off here (`canList`), so this guard is the
    // backstop rather than the mechanism: with no slug there is no row that
    // could be ours, and guessing would put another workspace's name on screen.
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
    // Thread-ref links embed this in durable text and cross-workspace routing
    // matches on it, so a rename must not move it.
    expect(workspaceName.value).toBe('personal');
  });
});
