import { describe, it, expect } from 'vitest';
import { backupSeed, refreshMayApply } from '../backupSeeding';
import type { BackupProviderInfo, BackupSchedule } from '../../../api/client';

function provider(id: string): BackupProviderInfo {
  return { id, name: id, connected: true, ready: true, missing_scopes: [], folder_url: null };
}

const AVAILABLE = [provider('google_drive'), provider('dropbox')];

function ok<T>(value: T): PromiseSettledResult<T> {
  return { status: 'fulfilled', value };
}

function failed<T>(): PromiseSettledResult<T> {
  return { status: 'rejected', reason: new Error('network') };
}

function schedule(over: Partial<BackupSchedule> = {}): BackupSchedule {
  return { provider: 'dropbox', schedule: '0 0 3 * * *', ...over };
}

const FRESH = { provider: '', providers: [] as BackupProviderInfo[] };
const RUNNING = { provider: 'dropbox', providers: AVAILABLE };

describe('backupSeed', () => {
  it('takes the configured destination over the registry order', () => {
    const seed = backupSeed(ok(AVAILABLE), ok(schedule()), FRESH);
    expect(seed.provider).toBe('dropbox');
    expect(seed.schedule).toBe('0 0 3 * * *');
    expect(seed.providers).toEqual(AVAILABLE);
  });

  it('reports an unknown schedule as null rather than off', () => {
    const seed = backupSeed(ok(AVAILABLE), failed<BackupSchedule>(), RUNNING);
    expect(seed.schedule).toBeNull();
  });

  it('keeps the current selection when the configured destination is unknown', () => {
    const seed = backupSeed(ok(AVAILABLE), failed<BackupSchedule>(), RUNNING);
    expect(seed.provider).toBe('dropbox');
  });

  it('still seeds something on mount when both reads failed', () => {
    const seed = backupSeed(ok(AVAILABLE), failed<BackupSchedule>(), FRESH);
    expect(seed.provider).toBe('google_drive');
    expect(backupSeed(failed<BackupProviderInfo[]>(), failed<BackupSchedule>(), FRESH).provider)
      .toBe('');
  });

  it('leaves the registry alone when its read failed', () => {
    const seed = backupSeed(failed<BackupProviderInfo[]>(), ok(schedule()), RUNNING);
    expect(seed.providers).toBeNull();
    expect(seed.provider).toBe('dropbox');
  });

  it('falls back when the configured destination is no longer offered', () => {
    const seed = backupSeed(ok(AVAILABLE), ok(schedule({ provider: 'ftp' })), RUNNING);
    expect(seed.provider).toBe('google_drive');
  });

  it('reads a cleared preference as the default rather than as unknown', () => {
    const seed = backupSeed(ok(AVAILABLE), ok({ provider: null, schedule: null }), RUNNING);
    expect(seed.provider).toBe('google_drive');
    expect(seed.schedule).toBe('off');
  });
});

describe('refreshMayApply', () => {
  const clean = { version: 2, applied: 1, writesAtStart: 0, writesNow: 0 };

  it('lets a newer read land', () => {
    expect(refreshMayApply(clean)).toBe(true);
  });

  it('drops a read older than what already landed', () => {
    expect(refreshMayApply({ ...clean, version: 1, applied: 2 })).toBe(false);
    expect(refreshMayApply({ ...clean, version: 2, applied: 2 })).toBe(false);
  });

  it('drops a read a local action started under', () => {
    expect(refreshMayApply({ ...clean, writesNow: 1 })).toBe(false);
  });
});
