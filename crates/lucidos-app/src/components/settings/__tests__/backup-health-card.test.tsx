import { describe, it, expect } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { backupHealthCard, shouldPollBackupStatus } from '../BackupSection';
import type { BackupStatus } from '../../../api/client';
import type { Loadable } from '../../../store/types';

/** Flatten a vnode tree into a string with the class + data-state attributes
 *  preserved so we can assert on per-state CSS classes. Mirrors the helper in
 *  directory-picker-loadable.test.tsx. */
function vnodeToText(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(vnodeToText).join('');
  const v = node as VNode<{ children?: ComponentChildren; class?: string; ['data-state']?: string }>;
  const tag = typeof v.type === 'string' ? v.type : '';
  const cls = v.props?.class ? ` class="${v.props.class}"` : '';
  const state = v.props?.['data-state'] ? ` data-state="${v.props['data-state']}"` : '';
  const inner = vnodeToText(v.props?.children);
  return tag ? `<${tag}${cls}${state}>${inner}</${tag}>` : inner;
}

function loaded(data: BackupStatus): Loadable<BackupStatus> {
  return { status: 'loaded', data };
}

const BASE: BackupStatus = {
  running: false,
  last_run: null,
  latest_backup: null,
  age_seconds: null,
  stale: false,
  list_error: null,
};

function recentBackup(ageSeconds: number) {
  return {
    id: 'b1',
    filename: 'lucidos-backup-personal-20260530-031500.enc',
    size_bytes: 42_000_000,
    created_at: new Date(Date.now() - ageSeconds * 1000).toISOString(),
  };
}

describe('backupHealthCard', () => {
  it('renders nothing before status loads (no flash)', () => {
    expect(backupHealthCard({ status: { status: 'not-loaded' }, liveProgress: null, providerName: 'Google Drive' })).toBeNull();
  });

  it('stale: a backup older than 24h renders the warning prominently', () => {
    const status = loaded({
      ...BASE,
      latest_backup: recentBackup(30 * 3600),
      age_seconds: 30 * 3600,
      stale: true,
    });
    const text = vnodeToText(backupHealthCard({ status, liveProgress: null, providerName: 'Google Drive' }));
    expect(text).toMatch(/data-state="stale"/);
    expect(text).toContain('backup-health-warn');
    expect(text).toContain('No successful backup in over 24 hours');
  });

  it('stale wording escalates past 48h and 72h', () => {
    const at48 = vnodeToText(backupHealthCard({
      status: loaded({ ...BASE, latest_backup: recentBackup(50 * 3600), age_seconds: 50 * 3600, stale: true }),
      liveProgress: null,
      providerName: 'Google Drive',
    }));
    expect(at48).toContain('No successful backup in over 48 hours');

    const at72 = vnodeToText(backupHealthCard({
      status: loaded({ ...BASE, latest_backup: recentBackup(80 * 3600), age_seconds: 80 * 3600, stale: true }),
      liveProgress: null,
      providerName: 'Google Drive',
    }));
    expect(at72).toContain('No successful backup in over 3 days');
  });

  it('no cloud backup at all renders a warning (data not backed up)', () => {
    const status = loaded({ ...BASE, latest_backup: null, age_seconds: null, stale: true });
    const text = vnodeToText(backupHealthCard({ status, liveProgress: null, providerName: 'Google Drive' }));
    expect(text).toMatch(/data-state="stale"/);
    expect(text).toContain('backup-health-warn');
  });

  it('fresh backup renders the cloud line without the warning', () => {
    const status = loaded({ ...BASE, latest_backup: recentBackup(3600), age_seconds: 3600, stale: false });
    const text = vnodeToText(backupHealthCard({ status, liveProgress: null, providerName: 'Google Drive' }));
    expect(text).toContain('Last cloud backup');
    expect(text).not.toContain('backup-health-warn');
    expect(text).not.toMatch(/data-state="stale"/);
  });

  it('last-run failure surfaces the error inside the red error span', () => {
    const status = loaded({
      ...BASE,
      last_run: { status: 'failure', at: new Date().toISOString(), filename: null, size_bytes: null, error: 'pg_dump failed' },
      latest_backup: recentBackup(3600),
      age_seconds: 3600,
      stale: false,
    });
    const text = vnodeToText(backupHealthCard({ status, liveProgress: null, providerName: 'Google Drive' }));
    expect(text).toContain('failed');
    // The error text must render in the red error span, not primary color.
    expect(text).toMatch(/backup-health-error">[^<]*pg_dump failed/);
  });

  it('last-run success shows a succeeded line', () => {
    const status = loaded({
      ...BASE,
      last_run: { status: 'success', at: new Date().toISOString(), filename: 'x.enc', size_bytes: 1, error: null },
      latest_backup: recentBackup(3600),
      age_seconds: 3600,
      stale: false,
    });
    const text = vnodeToText(backupHealthCard({ status, liveProgress: null, providerName: 'Google Drive' }));
    expect(text).toContain('succeeded');
  });

  it('running: live progress takes precedence over last-run', () => {
    const status = loaded({ ...BASE, running: true });
    const text = vnodeToText(backupHealthCard({
      status,
      liveProgress: { phase: 'encrypting', progress: 60, total: 100 },
      providerName: 'Google Drive',
    }));
    expect(text).toMatch(/data-state="running"/);
    expect(text).toContain('Backup in progress');
    expect(text).toContain('Encrypting');
  });

  it('list_error: surfaces a muted "couldn\'t reach" line, still renders', () => {
    const status = loaded({ ...BASE, list_error: 'Drive unreachable', stale: true });
    const text = vnodeToText(backupHealthCard({ status, liveProgress: null, providerName: 'Google Drive' }));
    expect(text).toContain('reach Google Drive to list backups');
  });
});

describe('shouldPollBackupStatus', () => {
  it('polls when the engine reports running but no live progress is flowing', () => {
    // The post-backup pruning window: backup_in_progress is still set after the
    // terminal SSE fired, so we must keep polling until it clears.
    expect(shouldPollBackupStatus(loaded({ ...BASE, running: true }), null)).toBe(true);
  });

  it('does not poll while live progress is flowing (the terminal SSE refreshes us)', () => {
    expect(shouldPollBackupStatus(loaded({ ...BASE, running: true }), { phase: 'encrypting', progress: 1, total: 100 })).toBe(false);
  });

  it('does not poll when not running', () => {
    expect(shouldPollBackupStatus(loaded({ ...BASE, running: false }), null)).toBe(false);
  });

  it('does not poll before status has loaded', () => {
    expect(shouldPollBackupStatus({ status: 'not-loaded' }, null)).toBe(false);
    expect(shouldPollBackupStatus({ status: 'loading' }, null)).toBe(false);
  });
});
