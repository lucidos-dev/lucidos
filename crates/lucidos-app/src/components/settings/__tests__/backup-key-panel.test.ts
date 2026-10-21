import { describe, it, expect } from 'vitest';
import { backupKeyStatusLine, backupsCanSeal } from '../BackupKeyPanel';
import type { BackupKeys } from '../../../api/client';

const NONE: BackupKeys = { recipient: null, pending: false, legacy_key: false, secs_since_verified: null };

/** The Backup page answers "what seals my backups?" from `/backup/recipient`.
 *  A pending key pair seals nothing until its backup key is pasted back, which
 *  is the paste-back gate seen from the page. */
describe('backupsCanSeal', () => {
  it('is false before setup, and stays false while a minted key waits', () => {
    expect(backupsCanSeal(NONE)).toBe(false);
    expect(backupsCanSeal({ ...NONE, pending: true })).toBe(false);
  });

  it('is true for a confirmed recipient or a legacy key', () => {
    expect(backupsCanSeal({ ...NONE, recipient: 'age1example' })).toBe(true);
    expect(backupsCanSeal({ ...NONE, legacy_key: true })).toBe(true);
  });
});

describe('backupKeyStatusLine', () => {
  it('asks for a backup key before setup', () => {
    expect(backupKeyStatusLine(NONE)).toContain('Backups are not set up');
  });

  it('prompts the switch while only a legacy key exists', () => {
    expect(backupKeyStatusLine({ ...NONE, legacy_key: true })).toContain('old backup key');
  });

  it('names only the waiting key while a switch from a legacy key is pending', () => {
    const line = backupKeyStatusLine({ ...NONE, legacy_key: true, pending: true });
    expect(line).toContain('waiting for you to paste it back');
    expect(line).not.toContain('old backup key');
  });

  it('keeps the current status beside a pending rotation', () => {
    const line = backupKeyStatusLine({ ...NONE, recipient: 'age1example', pending: true });
    expect(line).toContain('sealed to your backup key');
    expect(line).toContain('waiting for you to paste it back');
  });

  it('says when the backup key was last checked', () => {
    expect(backupKeyStatusLine({ ...NONE, recipient: 'age1example' })).toContain('not checked');
    expect(backupKeyStatusLine({ ...NONE, recipient: 'age1example', secs_since_verified: 3 * 86400 }))
      .toContain('3 days ago');
  });

  it('never shows the recipient or any key', () => {
    expect(backupKeyStatusLine({ ...NONE, recipient: 'age1example' })).not.toContain('age1');
  });
});
