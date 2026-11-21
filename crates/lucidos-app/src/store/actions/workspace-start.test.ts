import { describe, it, expect, beforeEach, vi } from 'vitest';
import { signal } from '@preact/signals';

const mockStartWorkspace = vi.fn();
vi.mock('../../api/client/control', () => ({
  startWorkspace: (...args: unknown[]) => mockStartWorkspace(...args),
}));
const mockCheckConnection = vi.fn();
const mockNoteStartAccepted = vi.fn();
vi.mock('./connection', () => ({
  checkConnection: (...args: unknown[]) => mockCheckConnection(...args),
  noteWorkspaceStartAccepted: () => {
    mockNoteStartAccepted();
    connectionStatus.value = 'starting';
  },
}));
const mockShowToast = vi.fn();
const connectionStatus = signal<string>('stopped');
vi.mock('../store', () => ({
  showToast: (...args: unknown[]) => mockShowToast(...args),
  connectionStatus,
}));

const { startThisWorkspace } = await import('./workspace-start');

beforeEach(() => {
  vi.clearAllMocks();
  connectionStatus.value = 'stopped';
});

describe('startThisWorkspace', () => {
  it('starts the workspace, reads starting, and resolves once the engine answers', async () => {
    mockStartWorkspace.mockResolvedValue(undefined);
    mockCheckConnection.mockResolvedValue(undefined);

    const result = startThisWorkspace('dev');
    await vi.waitFor(() => expect(mockCheckConnection).toHaveBeenCalledOnce());
    expect(connectionStatus.value).toBe('starting');
    connectionStatus.value = 'connected';

    expect(await result).toBe(true);
    expect(mockStartWorkspace).toHaveBeenCalledWith('dev');
    expect(mockNoteStartAccepted).toHaveBeenCalledOnce();
    expect(mockShowToast).not.toHaveBeenCalled();
  });

  it('tells the user when the gateway refuses, and neither reads starting nor probes', async () => {
    mockStartWorkspace.mockRejectedValue(new Error('workspace not found'));

    expect(await startThisWorkspace('dev')).toBe(false);

    expect(mockShowToast).toHaveBeenCalledOnce();
    expect(mockShowToast.mock.calls[0][0]).toContain('Could not start the workspace');
    expect(mockShowToast.mock.calls[0][1]).toBe('error');
    expect(mockNoteStartAccepted).not.toHaveBeenCalled();
    expect(mockCheckConnection).not.toHaveBeenCalled();
    expect(connectionStatus.value).toBe('stopped');
  });

  // The gateway boots in the background after it accepts a start. A boot that
  // fails there ends `starting` in something other than connected.
  it.each(['stopped', 'disconnected'])('tells the user when starting ends %s', async (end) => {
    mockStartWorkspace.mockResolvedValue(undefined);
    mockCheckConnection.mockImplementation(async () => {
      connectionStatus.value = end;
    });

    expect(await startThisWorkspace('dev')).toBe(false);
    expect(mockShowToast).toHaveBeenCalledOnce();
    expect(mockShowToast.mock.calls[0][0]).toContain('did not start');
    expect(mockShowToast.mock.calls[0][1]).toBe('error');
  });
});
