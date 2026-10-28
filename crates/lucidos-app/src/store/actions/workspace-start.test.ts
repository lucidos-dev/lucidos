import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { signal } from '@preact/signals';

const mockStartWorkspace = vi.fn();
vi.mock('../../api/client/control', () => ({
  startWorkspace: (...args: unknown[]) => mockStartWorkspace(...args),
}));
const mockCheckConnection = vi.fn();
vi.mock('./connection', () => ({
  checkConnection: (...args: unknown[]) => mockCheckConnection(...args),
}));
const mockShowToast = vi.fn();
const connectionStatus = signal<string>('stopped');
vi.mock('../store', () => ({
  showToast: (...args: unknown[]) => mockShowToast(...args),
  connectionStatus,
}));

const { startThisWorkspace, START_SETTLE_MS } = await import('./workspace-start');

beforeEach(() => {
  vi.clearAllMocks();
  connectionStatus.value = 'stopped';
});

afterEach(() => {
  vi.useRealTimers();
});

describe('startThisWorkspace', () => {
  it('starts the workspace, probes health at once, and resolves when it leaves stopped', async () => {
    mockStartWorkspace.mockResolvedValue(undefined);
    mockCheckConnection.mockImplementation(async () => {
      connectionStatus.value = 'disconnected';
    });

    expect(await startThisWorkspace('dev')).toBe(true);

    expect(mockStartWorkspace).toHaveBeenCalledWith('dev');
    expect(mockCheckConnection).toHaveBeenCalledOnce();
    expect(mockShowToast).not.toHaveBeenCalled();
  });

  it('tells the user when the gateway refuses, and does not probe', async () => {
    mockStartWorkspace.mockRejectedValue(new Error('workspace not found'));

    expect(await startThisWorkspace('dev')).toBe(false);

    expect(mockShowToast).toHaveBeenCalledOnce();
    expect(mockShowToast.mock.calls[0][0]).toContain('Could not start the workspace');
    expect(mockShowToast.mock.calls[0][1]).toBe('error');
    expect(mockCheckConnection).not.toHaveBeenCalled();
  });

  // The gateway boots in the background after it accepts a start. A boot that
  // fails there leaves the workspace stopped, with no error on the request.
  it('gives up and tells the user when the workspace stays stopped', async () => {
    vi.useFakeTimers();
    mockStartWorkspace.mockResolvedValue(undefined);
    mockCheckConnection.mockResolvedValue(undefined);

    const result = startThisWorkspace('dev');
    await vi.advanceTimersByTimeAsync(START_SETTLE_MS);

    expect(await result).toBe(false);
    expect(mockShowToast).toHaveBeenCalledOnce();
    expect(mockShowToast.mock.calls[0][0]).toContain('did not start');
    expect(mockShowToast.mock.calls[0][1]).toBe('error');
  });
});
