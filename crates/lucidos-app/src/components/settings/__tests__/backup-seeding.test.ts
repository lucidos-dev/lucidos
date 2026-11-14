/**
 * What the Backup page's provider and schedule controls settle on.
 *
 * Two callers now: the mount effect, and the SSE refresh that runs when a
 * backup preference changes elsewhere (the agent, another device, another tab).
 * The refresh is the reason these rules are a function rather than inline code:
 * a second, sloppier copy of them is how a provider change arriving over SSE
 * could select a destination the mount path would have rejected, or treat a
 * failed schedule read's 'off' default as a real answer and silently disable a
 * nightly backup.
 */
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
    // The page used to seed available[0] unconditionally, which is always Google
    // Drive, so an install configured for Dropbox rendered every control against
    // the wrong provider.
    const seed = backupSeed(ok(AVAILABLE), ok(schedule()), FRESH);
    expect(seed.provider).toBe('dropbox');
    expect(seed.schedule).toBe('0 0 3 * * *');
    expect(seed.providers).toEqual(AVAILABLE);
  });

  it('reports an unknown schedule as null rather than off', () => {
    // 'off' is a real answer meaning "no automatic backups". A failed read is
    // not that answer, and one endpoint writes the schedule and the provider
    // together, so a caller that conflated them could disable a nightly backup
    // as a side effect of a destination change.
    const seed = backupSeed(ok(AVAILABLE), failed<BackupSchedule>(), RUNNING);
    expect(seed.schedule).toBeNull();
  });

  it('keeps the current selection when the configured destination is unknown', () => {
    // The refresh case: nothing read said which provider is configured, so the
    // one on screen stays. Re-picking from the registry here is what would flip
    // a Dropbox install to Google Drive on a transient failure.
    const seed = backupSeed(ok(AVAILABLE), failed<BackupSchedule>(), RUNNING);
    expect(seed.provider).toBe('dropbox');
  });

  it('still seeds something on mount when both reads failed', () => {
    // Mount passes an empty current selection: the dropdown has to show
    // something, and the page reports the not-connected state for whatever it
    // lands on.
    const seed = backupSeed(ok(AVAILABLE), failed<BackupSchedule>(), FRESH);
    expect(seed.provider).toBe('google_drive');
    expect(backupSeed(failed<BackupProviderInfo[]>(), failed<BackupSchedule>(), FRESH).provider)
      .toBe('');
  });

  it('leaves the registry alone when its read failed', () => {
    // null means "this read did not answer". The mount path turns that into a
    // failed Loadable with the reason in it; the background refresh keeps the
    // last known good list and says nothing.
    const seed = backupSeed(failed<BackupProviderInfo[]>(), ok(schedule()), RUNNING);
    expect(seed.providers).toBeNull();
    // The selection is still validated, against the list already loaded.
    expect(seed.provider).toBe('dropbox');
  });

  it('falls back when the configured destination is no longer offered', () => {
    // A provider retired between releases, or a hand-edited preference.
    // Selecting it would leave every provider-scoped control disabled with
    // nothing on screen explaining why.
    const seed = backupSeed(ok(AVAILABLE), ok(schedule({ provider: 'ftp' })), RUNNING);
    expect(seed.provider).toBe('google_drive');
  });

  it('reads a cleared preference as the default rather than as unknown', () => {
    // `PreferencesChanged` with a null value is a reset to default, and
    // /backup/schedule then answers null for both fields. That IS an answer:
    // the configured destination is whatever the registry leads with, and
    // automatic backups are off.
    const seed = backupSeed(ok(AVAILABLE), ok({ provider: null, schedule: null }), RUNNING);
    expect(seed.provider).toBe('google_drive');
    expect(seed.schedule).toBe('off');
  });
});

/**
 * Which in-flight refresh is allowed to land.
 *
 * The refresh is asynchronous and more than one can be in the air: `PUT
 * /backup/schedule` writes two preferences, so one user action emits two
 * `PreferencesChanged` events and starts two refreshes. Whichever HTTP response
 * happens to arrive last is not necessarily the newest read.
 */
describe('refreshMayApply', () => {
  const clean = { version: 2, applied: 1, writesAtStart: 0, writesNow: 0 };

  it('lets a newer read land', () => {
    expect(refreshMayApply(clean)).toBe(true);
  });

  it('drops a read older than what already landed', () => {
    // The out-of-order case: refresh 2 finished first, then refresh 1 arrives
    // with staler values. Applying it would put the old destination back AND
    // record version 1 as applied, leaving nothing to trigger a correction.
    expect(refreshMayApply({ ...clean, version: 1, applied: 2 })).toBe(false);
    // Its own version already landed: a duplicate has nothing to add.
    expect(refreshMayApply({ ...clean, version: 2, applied: 2 })).toBe(false);
  });

  it('drops a read a local action started under', () => {
    // The user picked a provider (or pressed Grant access) while these reads
    // were out. Their result is older than the pick by construction.
    expect(refreshMayApply({ ...clean, writesNow: 1 })).toBe(false);
  });
});
