import { describe, expect, it } from 'vitest';
import type { CleanupRow } from '../../api/client/control';
import { cleanupTotals, isCleanupSuggested, sortCleanupRows } from './cleanupReview';

const now = Date.parse('2026-01-31T12:00:00Z');
function row(patch: Partial<CleanupRow> = {}): CleanupRow {
  return {
    id: 'sample', directory: '/workspaces/sample', state: 'stopped',
    workspace: { id: 'sample', name: 'Sample', dir: '/workspaces/sample', port: 9000,
      autostart: false, origin: 'eval', ephemeral: true, expires_at: null, last_opened_at: null },
    folder_bytes: 100, folder_modified: '2025-12-01T00:00:00Z', warning: null,
    database: { name: 'lucidos_sample', bytes: 50, last_event: '2025-12-01T00:00:00Z', warning: null },
    running: false, fingerprint: 'observed', ...patch,
  };
}

describe('cleanup review selection', () => {
  it('suggests only observed stopped harness resources with old activity or elapsed expiry', () => {
    expect(isCleanupSuggested(row(), now)).toBe(true);
    const expired = row();
    expired.workspace!.origin = 'demo';
    expired.workspace!.expires_at = '2026-01-30T00:00:00Z';
    expired.folder_modified = '2026-01-31T00:00:00Z';
    expect(isCleanupSuggested(expired, now)).toBe(true);
    for (const origin of ['unknown', 'user'] as const) {
      const protectedRow = row();
      protectedRow.workspace!.origin = origin;
      protectedRow.workspace!.expires_at = '2025-12-01T00:00:00Z';
      expect(isCleanupSuggested(protectedRow, now)).toBe(false);
    }
    for (const patch of [{ running: true }, { running: null }, { fingerprint: null },
      { workspace: null }, { warning: 'inventory unavailable' }]) {
      expect(isCleanupSuggested(row(patch), now)).toBe(false);
    }
  });

  it('uses the newest known activity and refuses unknown or invalid activity', () => {
    const recent = row();
    recent.workspace!.last_opened_at = '2026-01-30T00:00:00Z';
    expect(isCleanupSuggested(recent, now)).toBe(false);
    expect(isCleanupSuggested(row({ folder_modified: null, database: null }), now)).toBe(false);
    expect(isCleanupSuggested(row({ folder_modified: 'invalid' }), now)).toBe(false);
  });

  it('keeps unknown sizes explicit and sorts the review without mutating its source', () => {
    const rows = [row({ id: 'small' }), row({ id: 'unknown', folder_bytes: null }),
      row({ id: 'large', folder_bytes: 1000 })];
    expect(sortCleanupRows(rows).map((r) => r.id)).toEqual(['large', 'small', 'unknown']);
    expect(rows[0].id).toBe('small');
    expect(cleanupTotals(rows)).toEqual({ count: 3, knownBytes: 1250, unknownSizes: 1 });
    expect(cleanupTotals([row({ state: 'database-only', workspace: null,
      database: null, folder_bytes: null })]).unknownSizes).toBe(1);
  });
});
