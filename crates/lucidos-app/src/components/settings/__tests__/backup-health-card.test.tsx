import { describe, it, expect } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { backupHealthCard, backupHealthCardSkeleton, shouldPollBackupStatus, showBackupSetupOffer } from '../BackupSection';
import type { BackupProviderInfo, BackupStatus } from '../../../api/client';
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

/** Every `class` in the tree, component vnodes included. `vnodeToText` above
 *  only names host tags, so it cannot see the `Sk*` placeholder leaves (and
 *  invoking them here would throw: they read a hook). */
function collectClasses(node: ComponentChildren, out: string[] = []): string[] {
  if (node === null || node === undefined || typeof node !== 'object') return out;
  if (Array.isArray(node)) {
    for (const n of node) collectClasses(n, out);
    return out;
  }
  const v = node as VNode<{ children?: ComponentChildren; class?: string }>;
  if (v.props?.class) out.push(v.props.class);
  return collectClasses(v.props?.children, out);
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
    filename: 'lucidos-backup-myws-20260530-031500.enc',
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
      last_run: { status: 'failure', at: new Date().toISOString(), started_at: null, filename: null, size_bytes: null, error: 'pg_dump failed' },
      latest_backup: recentBackup(3600),
      age_seconds: 3600,
      stale: false,
    });
    const text = vnodeToText(backupHealthCard({ status, liveProgress: null, providerName: 'Google Drive' }));
    expect(text).toContain('failed');
    // The error text must render in the red error span, not primary color.
    expect(text).toMatch(/backup-health-error">[^<]*pg_dump failed/);
    // A failed last run escalates the whole card to the error (red) hue.
    expect(text).toMatch(/data-state="error"/);
  });

  it('a failed last run wins over staleness — the card is error, not stale', () => {
    // The user-visible scenario: the last attempt failed AND there's no recent
    // good backup. The card must read as an error (red), never sit inside the
    // soft-yellow stale box.
    const status = loaded({
      ...BASE,
      last_run: { status: 'failure', at: new Date().toISOString(), started_at: null, filename: null, size_bytes: null, error: 'Drive is full' },
      latest_backup: recentBackup(30 * 3600),
      age_seconds: 30 * 3600,
      stale: true,
    });
    const text = vnodeToText(backupHealthCard({ status, liveProgress: null, providerName: 'Google Drive' }));
    expect(text).toMatch(/data-state="error"/);
    expect(text).not.toMatch(/data-state="stale"/);
  });

  it('last-run success shows a succeeded line', () => {
    const status = loaded({
      ...BASE,
      last_run: { status: 'success', at: new Date().toISOString(), started_at: null, filename: 'x.enc', size_bytes: 1, error: null },
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

describe('backupHealthCardSkeleton', () => {
  const UNLOADED: Loadable<BackupStatus> = { status: 'not-loaded' };

  it('draws the card box, with a placeholder for each line the real card shows', () => {
    // The point of the skeleton: the card reserves its space instead of arriving
    // late and pushing the rest of the page down. Same box AND same line class
    // as the real card, so both take their metrics from one rule.
    const classes = collectClasses(backupHealthCardSkeleton());
    expect(classes).toContain('backup-health-card');
    expect(classes.filter((c) => c === 'backup-health-line')).toHaveLength(2);
  });

  it('shows the neutral hue, never a verdict it has not read', () => {
    expect(vnodeToText(backupHealthCardSkeleton())).toMatch(/data-state="idle"/);
  });

  it('stands in for a card that renders nothing on its own', () => {
    expect(backupHealthCard({ status: UNLOADED, liveProgress: null, providerName: '' })).toBeNull();
  });

  it('leaves the running card to draw itself while a refetch is in flight', () => {
    // The section gates its skeleton on the card having nothing to draw, because
    // both would land in the same grid cell. `loadStatus` blanks the status to
    // `loading` on every 4s poll of a running backup, and the card keeps
    // rendering live SSE progress through it: a shimmer stacked over that would
    // smear a placeholder across live information.
    const card = backupHealthCard({
      status: { status: 'loading' },
      liveProgress: { phase: 'uploading', progress: 30, total: 100 },
      providerName: 'Google Drive',
    });
    expect(card).not.toBeNull();
    expect(vnodeToText(card)).toMatch(/data-state="running"/);
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

describe('showBackupSetupOffer', () => {
  const PROVIDERS: Loadable<BackupProviderInfo[]> = {
    status: 'loaded',
    data: [{ id: 'google-drive', name: 'Google Drive', connected: true, ready: true, missing_scopes: [], folder_url: null }],
  };
  const WORKING = loaded({
    ...BASE,
    last_run: { status: 'success', at: new Date().toISOString(), started_at: null, filename: 'x.enc', size_bytes: 1, error: null },
    latest_backup: recentBackup(3600),
    age_seconds: 3600,
    stale: false,
  });

  it('hides the offer once a ready destination has a recent successful cloud backup', () => {
    expect(showBackupSetupOffer(PROVIDERS, true, WORKING)).toBe(false);
  });

  it('hides the offer with a good cloud backup and no run recorded this session', () => {
    const status = loaded({ ...BASE, latest_backup: recentBackup(3600), age_seconds: 3600 });
    expect(showBackupSetupOffer(PROVIDERS, true, status)).toBe(false);
  });

  it('shows the offer when no destination is ready, without waiting on status', () => {
    // Not connected, or connected but short of the upload scope. Status is never
    // fetched in that state, so it stays at not-loaded forever.
    expect(showBackupSetupOffer(PROVIDERS, false, { status: 'not-loaded' })).toBe(true);
  });

  it('shows the offer when nothing has ever been uploaded', () => {
    expect(showBackupSetupOffer(PROVIDERS, true, loaded(BASE))).toBe(true);
  });

  it('shows the offer when the last good backup has gone stale', () => {
    const status = loaded({
      ...BASE,
      latest_backup: recentBackup(30 * 3600),
      age_seconds: 30 * 3600,
      stale: true,
    });
    expect(showBackupSetupOffer(PROVIDERS, true, status)).toBe(true);
  });

  it('shows the offer when the last run failed, even with a good cloud backup', () => {
    const status = loaded({
      ...BASE,
      last_run: { status: 'failure', at: new Date().toISOString(), started_at: null, filename: null, size_bytes: null, error: 'quota' },
      latest_backup: recentBackup(3600),
      age_seconds: 3600,
    });
    expect(showBackupSetupOffer(PROVIDERS, true, status)).toBe(true);
  });

  it('stays hidden across a refetch of a working page (no flash on mount or poll)', () => {
    // loadStatus blanks the status to `loading` before every refetch, so reading
    // an unloaded status as "not working" would pop the offer in and shove the
    // health card down on every open and every 4s poll.
    expect(showBackupSetupOffer(PROVIDERS, true, { status: 'not-loaded' })).toBe(false);
    expect(showBackupSetupOffer(PROVIDERS, true, { status: 'loading' })).toBe(false);
    expect(showBackupSetupOffer(PROVIDERS, true, { status: 'failed', error: 'boom' })).toBe(false);
  });

  it('stays hidden while the destination registry has not landed', () => {
    expect(showBackupSetupOffer({ status: 'not-loaded' }, false, { status: 'not-loaded' })).toBe(false);
    expect(showBackupSetupOffer({ status: 'loading' }, false, { status: 'not-loaded' })).toBe(false);
    expect(showBackupSetupOffer({ status: 'failed', error: 'boom' }, false, { status: 'not-loaded' })).toBe(false);
  });

  it('stays hidden when the destination could not be listed', () => {
    // A transient cloud outage must not read as "your backups were never set
    // up"; the card says what actually happened in its own muted line.
    const status = loaded({
      ...BASE,
      latest_backup: recentBackup(3600),
      age_seconds: 3600,
      list_error: 'Drive unreachable',
      stale: true,
    });
    expect(showBackupSetupOffer(PROVIDERS, true, status)).toBe(false);
  });
});
