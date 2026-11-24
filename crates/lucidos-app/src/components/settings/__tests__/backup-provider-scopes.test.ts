import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import {
  PROVIDER_ACCESS,
  PROVIDER_SCOPES,
  backupAccessLine,
  oauthProviderFor,
  pickInitialProvider,
} from '../backupProviderScopes';

const BACKUP_PROVIDER_IDS = ['google_drive', 'dropbox'];

const here = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(here, '../../../../../..');

const GRANT_SCOPE_SOURCES: Record<string, string> = {
  google_drive: 'crates/lucidos-core/src/core/backup/google_drive.rs',
  dropbox: 'crates/lucidos-core/src/core/backup/dropbox.rs',
};

function rustGrantScopes(id: string): string[] {
  const file = GRANT_SCOPE_SOURCES[id];
  expect(file, `no GRANT_SCOPES source recorded for ${id}`).toBeTruthy();
  const src: string = readFileSync(resolve(REPO_ROOT, file), 'utf8');
  const start = src.indexOf('pub const GRANT_SCOPES: &[&str] = &[');
  expect(
    start,
    `could not find \`GRANT_SCOPES\` in ${file}. If it was renamed or moved, update this mirror rather than deleting it.`,
  ).toBeGreaterThan(-1);
  const scopes = [...src.slice(start, src.indexOf('];', start)).matchAll(/"([^"]+)"/g)].map(
    (m) => m[1],
  );
  expect(scopes.length, `\`GRANT_SCOPES\` in ${file} parsed as empty`).toBeGreaterThan(0);
  return scopes;
}

describe('the grant request mirrors the engine', () => {
  it('has a source file recorded for every provider', () => {
    expect(Object.keys(GRANT_SCOPE_SOURCES).sort()).toEqual([...BACKUP_PROVIDER_IDS].sort());
  });

  it.each(BACKUP_PROVIDER_IDS)('%s requests exactly its `GRANT_SCOPES`', (id) => {
    const requested = PROVIDER_SCOPES[id].split(' ').filter(Boolean);
    expect(requested.sort()).toEqual([...rustGrantScopes(id)].sort());
  });
});

describe('every backup provider can be granted access', () => {
  it.each(BACKUP_PROVIDER_IDS)('%s has a non-empty scope entry', (id) => {
    expect(PROVIDER_SCOPES[id]).toBeTruthy();
  });

  it('defines no scopes for a provider that does not exist', () => {
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
    expect(scopes).toContain('files.content.write');
  });

  it('uses Dropbox short scope names, not URLs', () => {
    for (const scope of scopes) expect(scope).not.toContain('://');
  });
});

describe('the access line under the Provider picker', () => {
  it('covers exactly the registered providers', () => {
    expect(Object.keys(PROVIDER_ACCESS).sort()).toEqual([...BACKUP_PROVIDER_IDS].sort());
  });

  it.each(BACKUP_PROVIDER_IDS)('%s names every scope it requests', (id) => {
    for (const scope of PROVIDER_SCOPES[id].split(' ')) {
      expect(PROVIDER_ACCESS[id]).toContain(scope.split('/').pop());
    }
  });

  it.each(BACKUP_PROVIDER_IDS)('%s says the archive is encrypted and how to revoke', (id) => {
    expect(PROVIDER_ACCESS[id]).toContain('encrypted before upload');
    expect(PROVIDER_ACCESS[id]).toContain('revoke access');
  });

  it('does not promise Dropbox stays in its app folder', () => {
    expect(PROVIDER_ACCESS.dropbox).toContain('App folder');
    expect(PROVIDER_ACCESS.dropbox).toContain('your whole Dropbox');
  });

  it('keeps Drive to the files Lucidos created', () => {
    expect(PROVIDER_SCOPES.google_drive).toBe('https://www.googleapis.com/auth/drive.file');
    expect(PROVIDER_ACCESS.google_drive).toContain('only the files it created');
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
    expect(backupAccessLine('Google Drive', ['https://www.googleapis.com/auth/drive.file'])).toBe(
      'Google Drive is missing the https://www.googleapis.com/auth/drive.file permission.',
    );
  });

  it('falls back to the bare sentence when the engine names nothing', () => {
    expect(backupAccessLine('Google Drive', [])).toBe(
      'Google Drive access not granted.',
    );
  });

  it('survives an engine too old to send the field at all', () => {
    expect(backupAccessLine('Dropbox', undefined)).toBe('Dropbox access not granted.');
  });
});

describe('which provider the Backup page opens on', () => {
  const REGISTRY = [{ id: 'google_drive' }, { id: 'dropbox' }];

  it('opens on the configured provider, not the first in the registry', () => {
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
    expect(pickInitialProvider('sftp', REGISTRY)).toBe('google_drive');
  });

  it('selects nothing when the registry is empty', () => {
    expect(pickInitialProvider('dropbox', [])).toBe('');
    expect(pickInitialProvider(null, [])).toBe('');
  });
});
