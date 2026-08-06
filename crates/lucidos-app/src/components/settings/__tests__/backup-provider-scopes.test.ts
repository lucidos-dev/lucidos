/**
 * The Grant access button on the Backup page can only ask for scopes this table
 * names. A provider missing from it has a button that does nothing useful, which
 * is what Dropbox shipped with: the engine reported it ready whatever its scopes
 * (`required_scope` was empty), so the dead button was unreachable and nobody
 * noticed until a backup failed at `files/create_folder_v2` with a 400 naming
 * `files.content.write` (2026-08-05).
 *
 * Both halves are pinned here: every registered provider has an entry, and the
 * entries carry the scopes the engine actually checks.
 */
import { describe, it, expect } from 'vitest';
import {
  PROVIDER_SCOPES,
  backupAccessLine,
  oauthProviderFor,
  pickInitialProvider,
} from '../backupProviderScopes';

/** Mirrors `PROVIDER_IDS` in `crates/lucidos-engine/src/core/backup/mod.rs`,
 *  which the engine's own `provider_ids_match_registry` test keeps in step with
 *  the registry. Adding a provider there means adding it here, which is the
 *  point: the failure is what tells you the new provider needs scopes. */
const BACKUP_PROVIDER_IDS = ['google_drive', 'dropbox'];

describe('every backup provider can be granted access', () => {
  it.each(BACKUP_PROVIDER_IDS)('%s has a non-empty scope entry', (id) => {
    expect(PROVIDER_SCOPES[id]).toBeTruthy();
  });

  it('defines no scopes for a provider that does not exist', () => {
    // A stale entry is dead weight the next reader has to reason about.
    expect(Object.keys(PROVIDER_SCOPES).sort()).toEqual([...BACKUP_PROVIDER_IDS].sort());
  });
});

describe('Dropbox asks for what a backup actually does', () => {
  const scopes = PROVIDER_SCOPES.dropbox.split(' ');

  it.each([
    ['files.content.write', 'create the folder, upload, and prune old backups'],
    ['files.content.read', 'download an archive when restoring'],
    ['files.metadata.read', 'list backups for retention and the health card'],
    ['account_info.read', 'name the connected account'],
  ])('requests %s to %s', (scope) => {
    expect(scopes).toContain(scope);
  });

  it('requests the scope the engine gates readiness on', () => {
    // `dropbox::REQUIRED_SCOPE` in the engine. If this one is not requested, the
    // provider can never become ready however many times Grant access is used.
    expect(scopes).toContain('files.content.write');
  });

  it('uses Dropbox short scope names, not URLs', () => {
    // Google's are full URLs; pasting that shape into a Dropbox authorize call
    // is rejected outright.
    for (const scope of scopes) expect(scope).not.toContain('://');
  });
});

describe('the OAuth provider behind each backup provider', () => {
  it('sends Drive through the Google account', () => {
    expect(oauthProviderFor('google_drive')).toBe('google');
  });

  it('leaves every other id alone', () => {
    expect(oauthProviderFor('dropbox')).toBe('dropbox');
  });
});

describe('what the page says when a connected provider cannot upload', () => {
  it('names the one permission the grant is short', () => {
    // The reported dead end: the user pressed Grant access, completed the
    // Dropbox consent screen, and came back to a line reading exactly what it
    // read before. Naming the scope is what distinguishes "you did not grant"
    // from "the App Console still does not permit this one".
    expect(backupAccessLine('Dropbox', ['files.metadata.read'])).toBe(
      'Dropbox is missing the files.metadata.read permission.',
    );
  });

  it('lists several, and agrees with itself about plurals', () => {
    expect(
      backupAccessLine('Dropbox', ['files.content.read', 'files.metadata.read']),
    ).toBe('Dropbox is missing the files.content.read, files.metadata.read permissions.');
  });

  it('renders whatever the engine named, without re-mapping it', () => {
    // The engine resolves its substring matchers to real scopes before they
    // reach the wire (`name_missing_scopes`), so Drive arrives already spelled
    // as the URL. A second mapping here would be a drifting answer to a question
    // already settled, and would disagree with `get_backup_status`.
    expect(backupAccessLine('Google Drive', ['https://www.googleapis.com/auth/drive.file'])).toBe(
      'Google Drive is missing the https://www.googleapis.com/auth/drive.file permission.',
    );
  });

  it('falls back to the bare sentence when the engine names nothing', () => {
    // A verdict that could not be resolved. Rendering an empty list would read
    // as a bug.
    expect(backupAccessLine('Google Drive', [])).toBe(
      'Google Drive access not granted.',
    );
  });

  it('survives an engine too old to send the field at all', () => {
    // The type says string[], but the wire does not: between a new bundle being
    // served and the engine restart landing, /backup/providers answers without
    // the key. A `.length` on that throws inside render and takes the whole
    // Settings view down with it.
    expect(backupAccessLine('Dropbox', undefined)).toBe('Dropbox access not granted.');
  });
});

describe('which provider the Backup page opens on', () => {
  // Registry order, which is where the old unconditional `available[0].id`
  // seed always landed.
  const REGISTRY = [{ id: 'google_drive' }, { id: 'dropbox' }];

  it('opens on the configured provider, not the first in the registry', () => {
    // The reported bug: an install configured for Dropbox rendered its health
    // card, ready verdict, Grant access and Back up now against Google Drive,
    // and a schedule change from there would have rewritten `backup_provider`
    // to a provider the user never picked.
    expect(pickInitialProvider('dropbox', REGISTRY)).toBe('dropbox');
  });

  it('opens on the configured provider even when it is already first', () => {
    expect(pickInitialProvider('google_drive', REGISTRY)).toBe('google_drive');
  });

  it.each([
    ['nothing configured', null],
    ['the preference unset', undefined],
    ['a blank preference', ''],
  ])('falls back to the first registered provider with %s', (_label, configured) => {
    expect(pickInitialProvider(configured, REGISTRY)).toBe('google_drive');
  });

  it('falls back when the configured provider is not in the registry', () => {
    // A retired provider, or a hand-edited preference. Selecting it would leave
    // every provider-scoped control disabled with nothing explaining why.
    expect(pickInitialProvider('sftp', REGISTRY)).toBe('google_drive');
  });

  it('selects nothing when the registry is empty', () => {
    // The providers request failed. The empty string is the component's own
    // "no provider" state, which already disables Back up now.
    expect(pickInitialProvider('dropbox', [])).toBe('');
    expect(pickInitialProvider(null, [])).toBe('');
  });
});
