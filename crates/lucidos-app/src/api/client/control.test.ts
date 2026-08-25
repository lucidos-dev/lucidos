import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  listWorkspaces,
  createWorkspace,
  restartWorkspace,
  stopWorkspace,
  restoreBackup,
  getGatewayStatus,
  reloadGateway,
  openWorkspace,
  openWorkspaceNotifications,
  slugifyWorkspaceName,
  parseWorkspaceNameFromArchive,
} from './control';
import { DEVICE_ID_KEY } from '../../utils/deviceIdHeader';

// The control plane lives under the reserved sigil namespace
// `/~/api/v1/control/*` (ADR 0014 §2) — an absolute path that can never collide
// with a workspace slug. These pin that the client targets the sigil path so a
// rename of the namespace can't silently break the picker.

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
  it('lists workspaces from the sigil control route', async () => {
    const mock = withFetch(() =>
      Promise.resolve(new Response('{"workspaces":[]}', { status: 200 })),
    );
    await listWorkspaces();
    expect(mock).toHaveBeenCalledTimes(1);
    expect(String(mock.mock.calls[0][0])).toBe('/~/api/v1/control/workspaces');
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
    // Multipart: the body is FormData (browser sets the boundary), not JSON.
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
    // `packaged` drives the picker's dev-only self-reload control gating.
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

  // Restart and Stop are the two control calls a PERSON makes about a running
  // engine, so they are the two that must say which device made them. The
  // gateway forwards the id to that engine right before it signals it, and it is
  // the only thing that tells the engine apart from a crash: without it the
  // interrupted threads settle at `failed` with "Response interrupted" and no
  // auto-resume instead of `paused` with "Paused by restart".
  it('sends the device-id header on restart so the engine can attribute the teardown', async () => {
    localStorage.setItem(DEVICE_ID_KEY, 'device-abc');
    const mock = withFetch(() => Promise.resolve(new Response(null, { status: 202 })));
    await restartWorkspace('dev');
    expect(String(mock.mock.calls[0][0])).toBe('/~/api/v1/control/workspaces/dev/restart');
    expect(mock.mock.calls[0][1]?.method).toBe('POST');
    expect(headersOf(mock)['x-lucidos-device-id']).toBe('device-abc');
  });

  it('sends the device-id header on stop', async () => {
    localStorage.setItem(DEVICE_ID_KEY, 'device-abc');
    const mock = withFetch(() => Promise.resolve(new Response(null, { status: 202 })));
    await stopWorkspace('dev');
    expect(String(mock.mock.calls[0][0])).toBe('/~/api/v1/control/workspaces/dev/stop');
    expect(headersOf(mock)['x-lucidos-device-id']).toBe('device-abc');
  });

  // A browser that has never opened a workspace has no id to send. It must not
  // mint one here (that would register a device nobody named); the restart just
  // goes through unattributed, exactly as it did before.
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

// These two helpers let the picker prefill a default restore name and predict a
// slug collision before uploading. They mirror the gateway's `registry::slugify`
// and `parse_workspace_name_from_archive` — keep them in lockstep.
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

// Regression pin for the reported bug: on the iOS PWA, a pane swipe fired
// WebKit's native back gesture and landed the user in the workspace they had
// switched away from. The switch had PUSHED a history entry, so back had
// somewhere to go. See `utils/documentNavigation.ts`.
describe('workspace navigation leaves nothing on the back stack', () => {
  // The node test env has no window.location (test-setup.ts aliases window to
  // globalThis), so it is installed as a global, as openExternalUrl.test.ts does.
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

  it('openWorkspaceNotifications replaces, and carries the landing hash', () => {
    openWorkspaceNotifications('loopws');
    expect(fakeLocation.replace).toHaveBeenCalledWith('/loopws/#notifications');
    expect(fakeLocation.href).toBe(CURRENT);
  });
});
