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
import { PROVIDER_SCOPES, oauthProviderFor } from '../backupProviderScopes';

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
