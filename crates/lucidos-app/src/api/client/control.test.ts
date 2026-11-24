import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  listWorkspaces,
  listWorkspacesWithOrder,
  setWorkspaceOrder,
  locateWorkspace,
  createWorkspace,
  restartWorkspace,
  startWorkspace,
  stopWorkspace,
  restoreBackup,
  getGatewayStatus,
  reloadGateway,
  openWorkspace,
  parseWorkspaceNameFromArchive,
  freezeWorkspaceDeletion,
  confirmCleanup,
  listTrash,
  restoreTrash,
  abandonTrashStaging,
  freezeEmptyTrash,
  confirmEmptyTrash,
} from './control';
import { slugifyWorkspaceName } from '../../utils/slug';
import { DEVICE_ID_KEY } from '../../utils/deviceIdHeader';


const originalFetch = globalThis.fetch;

function withFetch(impl: () => Promise<Response>): ReturnType<typeof vi.fn> {
  const mock = vi.fn().mockImplementation(impl);
  globalThis.fetch = mock as unknown as typeof fetch;
  return mock;
}

afterEach(() => {
  globalThis.fetch = originalFetch;
  localStorage.removeItem(DEVICE_ID_KEY);
  vi.restoreAllMocks();
});

function headersOf(mock: ReturnType<typeof vi.fn>, call = 0): Record<string, string> {
  return (mock.mock.calls[call][1]?.headers ?? {}) as Record<string, string>;
}

describe('control client', () => {
  it('reads Trash without masking errors', async () => {
    const mock = withFetch(() => Promise.resolve(new Response('{"error":"peer inventory unavailable"}', { status: 503 })));
    await expect(listTrash()).rejects.toThrow('peer inventory unavailable');
    expect(mock.mock.calls.map((call) => call[0])).toEqual([
      '/~/api/v1/control/trash',
    ]);
  });

  it('freezes exact workspace IDs and confirms the server phrase with the device identity', async () => {
    localStorage.setItem(DEVICE_ID_KEY, 'paired-device');
    const batch = {
      id: 'batch-test',
      phrase: 'Delete Sample',
      members: [{ id: 'sample', name: 'Sample', database_backup: false }],
    };
    const receipt = { results: [{ id: 'sample', status: 'deleted', detail: null }] };
    const mock = withFetch(() => Promise.resolve(Response.json(batch)));
    mock.mockResolvedValueOnce(Response.json(batch)).mockResolvedValueOnce(Response.json(receipt));
    expect(await freezeWorkspaceDeletion(['sample'])).toEqual(batch);
    expect(await confirmCleanup(batch.id, batch.phrase)).toEqual(receipt);
    expect(mock.mock.calls[0][0]).toBe('/~/api/v1/control/workspace-delete-batches');
    expect(JSON.parse(mock.mock.calls[0][1].body)).toEqual({ workspaces: ['sample'] });
    expect(JSON.parse(mock.mock.calls[1][1].body)).toEqual({ confirm: batch.phrase });
    expect(mock.mock.calls[1][0]).toBe('/~/api/v1/control/cleanup/batches/batch-test/confirm');
    for (let i = 0; i < 2; i++) {
      expect(mock.mock.calls[i][1].method).toBe('POST');
      expect(headersOf(mock, i)['x-lucidos-device-id']).toBe('paired-device');
    }
  });

  it('uses paired-device POSTs for Trash actions and escapes resource identities', async () => {
    localStorage.setItem(DEVICE_ID_KEY, 'paired-device');
    const mock = withFetch(() => Promise.resolve(new Response(null, { status: 204 })));
    await restoreTrash('workspace:example');
    await abandonTrashStaging('workspace:example');
    await freezeEmptyTrash();
    await confirmEmptyTrash('batch-test', 'Empty Trash');
    expect(mock.mock.calls.map((call) => call[0])).toEqual([
      '/~/api/v1/control/trash/workspace%3Aexample/restore',
      '/~/api/v1/control/trash/workspace%3Aexample/abandon-staging',
      '/~/api/v1/control/trash/empty-batches',
      '/~/api/v1/control/trash/empty-batches/batch-test/confirm',
    ]);
    for (let i = 0; i < 4; i++) {
      expect(mock.mock.calls[i][1].method).toBe('POST');
      expect(headersOf(mock, i)['x-lucidos-device-id']).toBe('paired-device');
    }
    expect(JSON.parse(mock.mock.calls[3][1].body)).toEqual({ confirm: 'Empty Trash' });
  });

  it('lists workspaces from the sigil control route', async () => {
    const mock = withFetch(() =>
      Promise.resolve(new Response('{"workspaces":[]}', { status: 200 })),
    );
    await listWorkspaces();
    expect(mock).toHaveBeenCalledTimes(1);
    expect(String(mock.mock.calls[0][0])).toBe('/~/api/v1/control/workspaces');
  });

  it('reads the workspace order beside the list, empty when the gateway sends none', async () => {
    withFetch(() => Promise.resolve(new Response('{"workspaces":[],"order":["b","a@cloud"]}', { status: 200 })));
    expect(await listWorkspacesWithOrder()).toEqual({ workspaces: [], order: ['b', 'a@cloud'] });
    withFetch(() => Promise.resolve(new Response('{"workspaces":[]}', { status: 200 })));
    expect(await listWorkspacesWithOrder()).toEqual({ workspaces: [], order: [] });
  });

  it('replaces the workspace order with a PUT to the sigil control route', async () => {
    const mock = withFetch(() => Promise.resolve(new Response(null, { status: 204 })));
    await setWorkspaceOrder(['b', 'a@cloud']);
    expect(String(mock.mock.calls[0][0])).toBe('/~/api/v1/control/workspace-order');
    expect(mock.mock.calls[0][1]?.method).toBe('PUT');
    expect(JSON.parse(mock.mock.calls[0][1].body)).toEqual({ order: ['b', 'a@cloud'] });
  });

  it('creates a workspace via POST to the sigil control route', async () => {
    const mock = withFetch(() =>
      Promise.resolve(
        new Response('{"workspace":{"id":"dev","name":"Dev","port":1,"health":"booting"}}', {
          status: 200,
        }),
      ),
    );
    const ws = await createWorkspace('Dev');
    expect(String(mock.mock.calls[0][0])).toBe('/~/api/v1/control/workspaces');
    expect(mock.mock.calls[0][1]?.method).toBe('POST');
    expect(ws.id).toBe('dev');
  });

  it('restores via multipart POST to the sigil restore route', async () => {
    const mock = withFetch(() =>
      Promise.resolve(new Response('{"id":"myws","name":"myws"}', { status: 200 })),
    );
    const file = new File([new Uint8Array([1, 2, 3])], 'lucidos-backup-myws-20260601-040254.enc');
    const started = await restoreBackup(file, 'a-key', 'myws');
    expect(String(mock.mock.calls[0][0])).toBe('/~/api/v1/control/workspaces/restore');
    expect(mock.mock.calls[0][1]?.method).toBe('POST');
    expect(mock.mock.calls[0][1]?.body).toBeInstanceOf(FormData);
    expect(started.id).toBe('myws');
  });

  it('reads gateway self-update status from the sigil control route', async () => {
    const mock = withFetch(() =>
      Promise.resolve(
        new Response('{"build_id":"abc123","update_available":true,"packaged":false}', { status: 200 }),
      ),
    );
    const status = await getGatewayStatus();
    expect(String(mock.mock.calls[0][0])).toBe('/~/api/v1/control/gateway/status');
    expect(status.update_available).toBe(true);
    expect(status.build_id).toBe('abc123');
    expect(status.packaged).toBe(false);
  });

  it('reports packaged=true from the gateway status (picker hides the dev reload control)', async () => {
    withFetch(() =>
      Promise.resolve(
        new Response('{"build_id":"abc123","update_available":false,"packaged":true}', { status: 200 }),
      ),
    );
    const status = await getGatewayStatus();
    expect(status.packaged).toBe(true);
  });

  it('reloads the gateway via POST to the sigil control route (202, bodyless)', async () => {
    const mock = withFetch(() => Promise.resolve(new Response(null, { status: 202 })));
    await reloadGateway();
    expect(String(mock.mock.calls[0][0])).toBe('/~/api/v1/control/gateway/reload');
    expect(mock.mock.calls[0][1]?.method).toBe('POST');
  });

  it('sends the device-id header on restart so the engine can attribute the teardown', async () => {
    localStorage.setItem(DEVICE_ID_KEY, 'device-abc');
    const mock = withFetch(() => Promise.resolve(new Response(null, { status: 202 })));
    await restartWorkspace('dev');
    expect(String(mock.mock.calls[0][0])).toBe('/~/api/v1/control/workspaces/dev/restart');
    expect(mock.mock.calls[0][1]?.method).toBe('POST');
    expect(headersOf(mock)['x-lucidos-device-id']).toBe('device-abc');
  });

  it('starts a workspace through its own start route, never restart', async () => {
    const mock = withFetch(() => Promise.resolve(new Response(null, { status: 202 })));
    await startWorkspace('dev');
    expect(String(mock.mock.calls[0][0])).toBe('/~/api/v1/control/workspaces/dev/start');
    expect(mock.mock.calls[0][1]?.method).toBe('POST');
  });

  it('sends the device-id header on stop', async () => {
    localStorage.setItem(DEVICE_ID_KEY, 'device-abc');
    const mock = withFetch(() => Promise.resolve(new Response(null, { status: 202 })));
    await stopWorkspace('dev');
    expect(String(mock.mock.calls[0][0])).toBe('/~/api/v1/control/workspaces/dev/stop');
    expect(headersOf(mock)['x-lucidos-device-id']).toBe('device-abc');
  });

  it('omits the header entirely when this browser has no device id yet', async () => {
    const mock = withFetch(() => Promise.resolve(new Response(null, { status: 202 })));
    await restartWorkspace('dev');
    expect(headersOf(mock)['x-lucidos-device-id']).toBeUndefined();
    expect(localStorage.getItem(DEVICE_ID_KEY)).toBeNull();
  });

  it('surfaces a 409 collision as an Error carrying the server message', async () => {
    withFetch(() =>
      Promise.resolve(
        new Response('{"error":"A workspace named \\"myws\\" already exists — choose a different name."}', {
          status: 409,
        }),
      ),
    );
    const file = new File([new Uint8Array([1])], 'lucidos-backup-myws-20260601-040254.enc');
    await expect(restoreBackup(file, 'a-key')).rejects.toThrow(/already exists/);
  });
});

describe('restore name helpers', () => {
  it('slugifyWorkspaceName mirrors the registry slug rules', () => {
    expect(slugifyWorkspaceName('Default')).toBe('default');
    expect(slugifyWorkspaceName('Work 💼')).toBe('work');
    expect(slugifyWorkspaceName('My Cool Workspace!!')).toBe('my-cool-workspace');
    expect(slugifyWorkspaceName('a___b---c')).toBe('a-b-c');
    expect(slugifyWorkspaceName('')).toBe('workspace');
    expect(slugifyWorkspaceName('---')).toBe('workspace');
  });

  it('parseWorkspaceNameFromArchive extracts the name or returns null', () => {
    expect(parseWorkspaceNameFromArchive('lucidos-backup-myws-20260601-040254.enc')).toBe('myws');
    expect(parseWorkspaceNameFromArchive('lucidos-backup-e2e-test-20260601-040254.enc')).toBe('e2e-test');
    expect(parseWorkspaceNameFromArchive('random.enc')).toBeNull();
    expect(parseWorkspaceNameFromArchive('lucidos-backup-myws.enc')).toBeNull();
    expect(parseWorkspaceNameFromArchive('lucidos-backup-myws-20260601.enc')).toBeNull();
  });
});

describe('locateWorkspace', () => {
  const PEER = {
    status: 'reachable',
    install: 'Lucidos.app in /Applications',
    gateway_port: 5252,
    slug: 'work',
  };

  it('asks the sigil control route, by name', async () => {
    const mock = withFetch(() =>
      Promise.resolve(new Response(JSON.stringify(PEER), { status: 200 })),
    );
    await expect(locateWorkspace('My Space')).resolves.toEqual(PEER);
    expect(mock.mock.calls[0][0]).toBe(
      '/~/api/v1/control/workspace-location?name=My%20Space',
    );
  });

  it('reads a 404 as "no other install carries it"', async () => {
    withFetch(() => Promise.resolve(new Response('{"error":"nope"}', { status: 404 })));
    await expect(locateWorkspace('ghost')).resolves.toBeNull();
  });

  it('reads an older gateway\'s picker shell as "cannot say"', async () => {
    withFetch(() =>
      Promise.resolve(
        new Response('<!DOCTYPE html><html><head><base href="/~/">', {
          status: 200,
          headers: { 'content-type': 'text/html; charset=utf-8' },
        }),
      ),
    );
    await expect(locateWorkspace('work')).resolves.toBeNull();
  });

  it('throws on a real failure, so the caller can name the cause', async () => {
    withFetch(() => Promise.resolve(new Response('{"error":"boom"}', { status: 500 })));
    await expect(locateWorkspace('work')).rejects.toThrow(/boom/);
  });
});

describe('workspace navigation leaves nothing on the back stack', () => {
  const CURRENT = 'https://gateway.example.com/dev/';
  let fakeLocation: { href: string; replace: ReturnType<typeof vi.fn> };

  beforeEach(() => {
    fakeLocation = { href: CURRENT, replace: vi.fn() };
    vi.stubGlobal('location', fakeLocation);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('openWorkspace replaces the current history entry', () => {
    openWorkspace('loopws');
    expect(fakeLocation.replace).toHaveBeenCalledWith('/loopws/');
  });

  it('openWorkspace never assigns location.href, which would push', () => {
    openWorkspace('loopws');
    expect(fakeLocation.href).toBe(CURRENT);
  });

  it('openWorkspace encodes a slug that needs it', () => {
    openWorkspace('a b');
    expect(fakeLocation.replace).toHaveBeenCalledWith('/a%20b/');
  });

  it('openWorkspace carries a landing as the fragment, and still replaces', () => {
    openWorkspace('loopws', 'notifications');
    expect(fakeLocation.replace).toHaveBeenCalledWith('/loopws/#notifications');
    expect(fakeLocation.href).toBe(CURRENT);
  });
});
