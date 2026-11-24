
import { describe, it, expect } from 'vitest';
import type { WorkspaceStatus } from '../../../api/client/control';
import {
  applyRestoreFile,
  backupNote,
  collidingWorkspace,
  createNote,
  isFirstRun,
  nameTakenBy,
  nameTakenMessage,
  restoreBlocker,
  restoreFileNote,
  showsAddress,
  EMPTY_RESTORE_DRAFT,
  type RestoreDraft,
} from '../workspaceForms';

function ws(id: string, name = id): WorkspaceStatus {
  return { id, name, port: 5000, health: 'healthy', autostart: true };
}

function file(name: string): File {
  return new File(['x'], name);
}

const RENAMED = [ws('personal', 'personaal')];

const READY: RestoreDraft = { file: file('backup.enc'), key: 'k', name: 'fresh' };

describe('restoreBlocker: a disabled Restore always says what is missing', () => {
  it('names the next missing field, one at a time', () => {
    expect(restoreBlocker(EMPTY_RESTORE_DRAFT, [])?.kind).toBe('file');
    expect(restoreBlocker({ ...READY, file: null }, [])?.kind).toBe('file');
    expect(restoreBlocker({ ...READY, key: '  ' }, [])?.kind).toBe('key');
    expect(restoreBlocker({ ...READY, name: '  ' }, [])?.kind).toBe('name');
  });

  it('returns null only when the restore can actually start', () => {
    expect(restoreBlocker(READY, RENAMED)).toBeNull();
  });

  it('mentions the backup key by the name the user knows it by', () => {
    expect(restoreBlocker({ ...READY, key: '' }, [])?.message).toContain('backup key');
  });
});

describe('collision: the message names a workspace the user can see', () => {
  it('resolves the collision by ADDRESS and reports the display name', () => {
    const blocker = restoreBlocker({ ...READY, name: 'personal' }, RENAMED);
    expect(blocker?.kind).toBe('collision');
    expect(blocker?.message).not.toContain('“personal” already exists');
    expect(blocker?.message).toContain('personaal');
    expect(blocker?.message).toContain('/personal/');
  });

  it('carries the colliding workspace so the UI can offer to delete THAT one', () => {
    const blocker = restoreBlocker({ ...READY, name: 'Personal' }, RENAMED);
    expect(blocker?.kind === 'collision' && blocker.existing.id).toBe('personal');
  });

  it('collides on the slugified name, not on string equality', () => {
    expect(collidingWorkspace('My Work', [ws('my-work', 'Something else')])?.id).toBe('my-work');
    expect(collidingWorkspace('  ', RENAMED)).toBeNull();
    expect(collidingWorkspace('personaal', RENAMED)).toBeNull();
  });
});

describe('applyRestoreFile: an empty selection is never a reset', () => {
  it('keeps the chosen file when the dialog is cancelled', () => {
    const picked = applyRestoreFile(EMPTY_RESTORE_DRAFT, file('lucidos-backup-personal-20260601-040254.enc'));
    expect(picked.file?.name).toContain('personal');
    expect(picked.name).toBe('personal');
    expect(applyRestoreFile(picked, null)).toBe(picked);
    expect(applyRestoreFile(picked, undefined)).toBe(picked);
  });

  it('refills the name from a recognizable archive, keeps a typed one otherwise', () => {
    const typed: RestoreDraft = { ...EMPTY_RESTORE_DRAFT, name: 'my choice' };
    expect(applyRestoreFile(typed, file('renamed-download.enc')).name).toBe('my choice');
    expect(applyRestoreFile(typed, file('lucidos-backup-work-20260601-040254.enc')).name).toBe('work');
    expect(applyRestoreFile(typed, file('lucidos-backup-work-20260601-040254.age')).name).toBe('work');
  });
});

describe('restoreFileNote: warns about a non-archive without blocking it', () => {
  it('flags a file that is neither .age nor .enc', () => {
    expect(restoreFileNote(file('holiday.jpg'))).toContain('holiday.jpg');
    expect(restoreFileNote(file('BACKUP.ENC'))).toBeNull();
    expect(restoreFileNote(file('backup.age'))).toBeNull();
    expect(restoreFileNote(null)).toBeNull();
  });

  it('does not block submission', () => {
    expect(restoreBlocker({ ...READY, file: file('renamed') }, [])).toBeNull();
  });
});

describe('createNote: the silent -2 suffix is stated up front', () => {
  it('predicts the address a colliding create actually gets, without blocking', () => {
    const note = createNote('personal', RENAMED);
    expect(note?.blocking).toBe(false);
    expect(note?.message).toContain('personaal');
    expect(note?.message).toContain('/personal/');
    expect(note?.message).toContain('/personal-2/');
  });

  it('counts the suffixed addresses already taken', () => {
    const note = createNote('work', [ws('work', 'Alpha'), ws('work-2', 'Beta')]);
    expect(note?.blocking).toBe(false);
    expect(note?.message).toContain('/work-3/');
  });

  it('says nothing when the address is free', () => {
    expect(createNote('fresh', RENAMED)).toBeNull();
    expect(createNote('', RENAMED)).toBeNull();
  });
});

describe('display names are unique', () => {
  it('blocks a create whose name another workspace already carries', () => {
    const note = createNote('personaal', RENAMED);
    expect(note?.blocking).toBe(true);
    expect(note?.message).toContain('personaal');
  });

  it('matches a taken name across case and padding', () => {
    for (const probe of ['personaal', 'PersonAAL', '  personaal  ']) {
      expect(nameTakenBy(probe, RENAMED)?.id, probe).toBe('personal');
    }
    expect(nameTakenBy('something else', RENAMED)).toBeNull();
    expect(nameTakenBy('   ', RENAMED)).toBeNull();
  });

  it('lets a workspace keep its own name on rename', () => {
    expect(nameTakenBy('personaal', RENAMED, 'personal')).toBeNull();
    expect(nameTakenBy('PERSONAAL', RENAMED, 'personal')).toBeNull();
    const two = [ws('a', 'Alpha'), ws('b', 'Beta')];
    expect(nameTakenBy('Beta', two, 'a')?.id).toBe('b');
  });

  it('blocks a restore on the name before it talks about the address', () => {
    const draft: RestoreDraft = { file: file('b.enc'), key: 'k', name: 'personaal' };
    const blocker = restoreBlocker(draft, RENAMED);
    expect(blocker?.kind).toBe('collision');
    expect(blocker?.message).toContain('already have a workspace called');
  });

  it('quotes the existing name as stored, not as typed', () => {
    expect(nameTakenMessage(RENAMED[0])).toContain('“personaal”');
  });
});

describe('isFirstRun: only a loaded, empty list', () => {
  it('is the first run when the list loaded with nothing in it', () => {
    expect(isFirstRun({ status: 'loaded', data: [] })).toBe(true);
  });

  it('is not the first run once a workspace exists', () => {
    expect(isFirstRun({ status: 'loaded', data: [ws('personal')] })).toBe(false);
  });

  it('is not the first run while the list is still loading', () => {
    expect(isFirstRun({ status: 'not-loaded' })).toBe(false);
    expect(isFirstRun({ status: 'loading' })).toBe(false);
    expect(isFirstRun({ status: 'failed', error: 'boom' })).toBe(false);
  });
});

describe('showsAddress: quiet unless the address would surprise', () => {
  it('shows it when a rename moved the name off its address', () => {
    expect(showsAddress(RENAMED[0], RENAMED)).toBe(true);
  });

  it('shows it on BOTH rows when two workspaces share a display name', () => {
    const dupes = [ws('personal', 'personal'), ws('personal-2', 'personal')];
    expect(dupes.every((w) => showsAddress(w, dupes))).toBe(true);
  });

  it('stays quiet in the ordinary case', () => {
    const plain = [ws('personal'), ws('work')];
    expect(plain.some((w) => showsAddress(w, plain))).toBe(false);
  });
});

describe('backupNote: silent only when the gateway could not ask', () => {
  function backedUp(
    line: WorkspaceStatus['last_successful_backup'],
  ): WorkspaceStatus {
    return { ...ws('personal'), last_successful_backup: line };
  }

  it('says nothing when the field is absent', () => {
    expect(backupNote(ws('personal'))).toBeNull();
  });

  it('reads a recent backup as reassurance', () => {
    const at = new Date(Date.now() - 3 * 3600_000).toISOString();
    expect(backupNote(backedUp({ at, stale: false, configured: true }))).toEqual({
      text: 'Backed up 3h ago',
      level: 'ok',
    });
  });

  it('warns on a backup the ENGINE called stale, without re-deriving why', () => {
    const at = new Date(Date.now() - 3 * 86_400_000).toISOString();
    expect(backupNote(backedUp({ at, stale: true, configured: true }))).toEqual({
      text: 'Backed up 3d ago',
      level: 'warn',
    });
  });

  it('tells a broken schedule from a workspace nobody set up', () => {
    expect(backupNote(backedUp({ at: null, stale: true, configured: true }))).toEqual({
      text: 'Never backed up',
      level: 'warn',
    });
    expect(backupNote(backedUp({ at: null, stale: true, configured: false }))).toEqual({
      text: 'Not backed up',
      level: 'warn',
    });
  });
});
