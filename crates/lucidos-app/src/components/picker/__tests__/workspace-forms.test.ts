/**
 * The picker's form rules: what the user is told, and when.
 *
 * The bugs these pin are all "the UI knew something and didn't say it": a
 * disabled Restore with no stated reason, a collision message naming a
 * workspace nobody can see, a cancelled file dialog silently emptying the form,
 * and a create that quietly lands somewhere else than the name suggests.
 */

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

/** The reported case: the workspace on screen reads "personaal", but its
 *  address (frozen when it was created as "personal") is still /personal/. */
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
    // "Enter the key" was the old placeholder-only state: nothing said which
    // key, or that it was the one saved at backup-setup time.
    expect(restoreBlocker({ ...READY, key: '' }, [])?.message).toContain('backup key');
  });
});

describe('collision: the message names a workspace the user can see', () => {
  it('resolves the collision by ADDRESS and reports the display name', () => {
    const blocker = restoreBlocker({ ...READY, name: 'personal' }, RENAMED);
    expect(blocker?.kind).toBe('collision');
    // The whole point: never assert the existence of a name no row shows.
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
    expect(collidingWorkspace('personaal', RENAMED)).toBeNull(); // free address
  });
});

describe('applyRestoreFile: an empty selection is never a reset', () => {
  it('keeps the chosen file when the dialog is cancelled', () => {
    // The reported "Restore is disabled and I can't see why": reopening the
    // dialog and cancelling used to clear both the file and the name.
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
  });
});

describe('restoreFileNote: warns about a non-archive without blocking it', () => {
  it('flags a file that is not .enc', () => {
    expect(restoreFileNote(file('holiday.jpg'))).toContain('holiday.jpg');
    expect(restoreFileNote(file('BACKUP.ENC'))).toBeNull();
    expect(restoreFileNote(null)).toBeNull();
  });

  it('does not block submission', () => {
    // A user who renamed their download must still be able to restore it; the
    // gateway is the authority on whether the archive decrypts.
    expect(restoreBlocker({ ...READY, file: file('renamed') }, [])).toBeNull();
  });
});

describe('createNote: the silent -2 suffix is stated up front', () => {
  it('predicts the address a colliding create actually gets, without blocking', () => {
    // The NAME "personal" is free here (the workspace is called "personaal"),
    // only its address is taken, so the create is allowed and merely explained.
    const note = createNote('personal', RENAMED);
    expect(note?.blocking).toBe(false);
    expect(note?.message).toContain('personaal');
    expect(note?.message).toContain('/personal/');
    expect(note?.message).toContain('/personal-2/');
  });

  it('counts the suffixed addresses already taken', () => {
    // Both addresses taken, both by workspaces named something else, so the
    // name is free and only the address needs explaining.
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
    // The reported picker: two rows both reading "personaaa". The address chip
    // made that legible; it should not have been creatable at all.
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
    // Taking a DIFFERENT workspace's name still collides.
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
    // A loading list has no data yet for a reason that says nothing about what
    // the user has. Reading it as the first run flashes the name chips and the
    // unfolded create form at a user who has five workspaces.
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
  /** A row carrying the engine's backup answer. */
  function backedUp(
    line: WorkspaceStatus['last_successful_backup'],
  ): WorkspaceStatus {
    return { ...ws('personal'), last_successful_backup: line };
  }

  it('says nothing when the field is absent', () => {
    // A stopped or unhealthy workspace, and an engine too old to answer. The
    // gateway holds no database handle, so it genuinely does not know, and a
    // guess here would call a nightly-backed-up workspace unprotected.
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
    // The threshold lives once, in core::backup. The same timestamp with
    // `stale: false` is the case above, so nothing here reads the clock.
    const at = new Date(Date.now() - 3 * 86_400_000).toISOString();
    expect(backupNote(backedUp({ at, stale: true, configured: true }))).toEqual({
      text: 'Backed up 3d ago',
      level: 'warn',
    });
  });

  it('tells a broken schedule from a workspace nobody set up', () => {
    // Both are unprotected and both warn, but they are different faults: one
    // has a schedule that has never produced an archive, the other has none.
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
