import { describe, it, expect, vi, afterEach } from 'vitest';
import {
  listWorkspaces,
  createWorkspace,
  restoreBackup,
  slugifyWorkspaceName,
  parseWorkspaceNameFromArchive,
} from './control';

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
  vi.restoreAllMocks();
});

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
      Promise.resolve(new Response('{"id":"personal","name":"personal"}', { status: 200 })),
    );
    const file = new File([new Uint8Array([1, 2, 3])], 'lucidos-backup-personal-20260601-040254.enc');
    const started = await restoreBackup(file, 'a-key', 'personal');
    expect(String(mock.mock.calls[0][0])).toBe('/~/api/v1/control/workspaces/restore');
    expect(mock.mock.calls[0][1]?.method).toBe('POST');
    // Multipart: the body is FormData (browser sets the boundary), not JSON.
    expect(mock.mock.calls[0][1]?.body).toBeInstanceOf(FormData);
    expect(started.id).toBe('personal');
  });

  it('surfaces a 409 collision as an Error carrying the server message', async () => {
    withFetch(() =>
      Promise.resolve(
        new Response('{"error":"A workspace named \\"personal\\" already exists — choose a different name."}', {
          status: 409,
        }),
      ),
    );
    const file = new File([new Uint8Array([1])], 'lucidos-backup-personal-20260601-040254.enc');
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
    expect(parseWorkspaceNameFromArchive('lucidos-backup-personal-20260601-040254.enc')).toBe('personal');
    expect(parseWorkspaceNameFromArchive('lucidos-backup-e2e-test-20260601-040254.enc')).toBe('e2e-test');
    expect(parseWorkspaceNameFromArchive('random.enc')).toBeNull();
    expect(parseWorkspaceNameFromArchive('lucidos-backup-personal.enc')).toBeNull();
    expect(parseWorkspaceNameFromArchive('lucidos-backup-personal-20260601.enc')).toBeNull();
  });
});
