import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as apiClient from '../../api/client';
import { ApiError } from '../../api/client';
import { backupProgress, lastSuccessfulBackup, toasts } from '../store';
import { loadLastSuccessfulBackup, startBackup } from './backup';

const ANSWER = { at: '2026-11-01T09:00:00Z', stale: true, configured: true, overdue: true, overdue_at: null };

beforeEach(() => {
  backupProgress.value = null;
  toasts.value = [];
  lastSuccessfulBackup.value = { status: 'not-loaded' };
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('startBackup, the one start path for Settings and the overdue banner', () => {
  it('marks the backup as starting and leaves the end to the SSE events', async () => {
    const spy = vi.spyOn(apiClient, 'createBackup').mockResolvedValue();
    await startBackup('google_drive');
    expect(spy).toHaveBeenCalledWith('google_drive');
    expect(backupProgress.value).toEqual({ phase: 'starting', progress: 0, total: 0 });
  });

  // The engine answers 409 for a running backup, a snapshot job, missing setup
  // and a provider that is not ready. Each message says what to do.
  it.each([
    'Backup already in progress',
    'Google Drive is not ready: grant access first',
  ])('shows the engine\'s 409 message "%s" and clears the progress', async (message) => {
    vi.spyOn(apiClient, 'createBackup').mockRejectedValue(new ApiError(409, message));
    await startBackup('google_drive');
    expect(backupProgress.value).toBeNull();
    expect(toasts.value.map((t) => t.message)).toEqual([message]);
  });

  it('toasts any other failure and clears the progress', async () => {
    vi.spyOn(apiClient, 'createBackup').mockRejectedValue(new Error('disk full'));
    await startBackup('google_drive');
    expect(backupProgress.value).toBeNull();
    expect(toasts.value[0].message).toContain('Backup failed:');
  });
});

describe('loadLastSuccessfulBackup feeds the banner', () => {
  it('loads the engine verdict', async () => {
    vi.spyOn(apiClient, 'getLastSuccessfulBackup').mockResolvedValue(ANSWER);
    await loadLastSuccessfulBackup();
    expect(lastSuccessfulBackup.value).toEqual({ status: 'loaded', data: ANSWER });
  });

  it('keeps the loaded answer on screen while it re-reads', async () => {
    lastSuccessfulBackup.value = { status: 'loaded', data: ANSWER };
    let seenDuringRead: string | undefined;
    vi.spyOn(apiClient, 'getLastSuccessfulBackup').mockImplementation(async () => {
      seenDuringRead = lastSuccessfulBackup.value.status;
      return { ...ANSWER, overdue: false };
    });
    await loadLastSuccessfulBackup();
    expect(seenDuringRead).toBe('loaded');
    expect(lastSuccessfulBackup.value).toMatchObject({ status: 'loaded', data: { overdue: false } });
  });

  it('a failed re-read replaces the old answer, so a stale overdue cannot outlive a backup', async () => {
    lastSuccessfulBackup.value = { status: 'loaded', data: ANSWER };
    vi.spyOn(apiClient, 'getLastSuccessfulBackup').mockRejectedValue(new ApiError(500, 'boom'));
    await loadLastSuccessfulBackup();
    expect(lastSuccessfulBackup.value.status).toBe('failed');
  });

  it('an older read landing last cannot bring back a cleared overdue', async () => {
    let releaseOld!: () => void;
    const spy = vi.spyOn(apiClient, 'getLastSuccessfulBackup');
    spy.mockImplementationOnce(() => new Promise((resolve) => { releaseOld = () => resolve(ANSWER); }));
    spy.mockResolvedValueOnce({ ...ANSWER, overdue: false });
    const older = loadLastSuccessfulBackup();
    await loadLastSuccessfulBackup();
    releaseOld();
    await older;
    expect(lastSuccessfulBackup.value).toMatchObject({ status: 'loaded', data: { overdue: false } });
  });
});
