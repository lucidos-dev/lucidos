import { describe, it, expect } from 'vitest';
import { backupKeyButtonLabel } from '../BackupSection';

/** The key button must NEVER imply a destructive action. These cases pin the
 *  user-reported confusion: a "show" action that silently minted a key surfaced
 *  a "New backup key generated" toast for a workspace that already had backups.
 *  The fix splits the label: "Generate new backup key" only when none exists,
 *  "Show backup key" when one does. */
describe('backupKeyButtonLabel', () => {
  it('says "Generate new backup key" only when a key is known to be absent', () => {
    expect(backupKeyButtonLabel(false, false)).toBe('Generate new backup key');
  });

  it('says "Show backup key" when a key exists and is hidden', () => {
    expect(backupKeyButtonLabel(true, false)).toBe('Show backup key');
  });

  it('says "Hide backup key" while a key is revealed, regardless of existence', () => {
    expect(backupKeyButtonLabel(true, true)).toBe('Hide backup key');
    expect(backupKeyButtonLabel(false, true)).toBe('Hide backup key');
    expect(backupKeyButtonLabel(null, true)).toBe('Hide backup key');
  });

  it('defaults to the non-destructive "Show backup key" while existence is unknown', () => {
    // null = the on-mount existence probe has not answered yet. We must not
    // guess "Generate" (which would imply minting); the click handler reveals
    // and only falls back to generate on a real 404.
    expect(backupKeyButtonLabel(null, false)).toBe('Show backup key');
  });
});
