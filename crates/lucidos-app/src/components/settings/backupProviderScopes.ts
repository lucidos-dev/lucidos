/**
 * Which OAuth scopes each backup provider needs, and which OAuth provider it
 * connects through.
 *
 * Its own module (importing nothing) so the values can be unit-tested directly:
 * `BackupSection.tsx` pulls in the backup API, OAuth state and live SSE
 * progress, none of which a table of scope strings should need.
 */

/** OAuth scopes to request when granting a backup provider access.
 *
 *  Every provider the engine's backup registry can report MUST have an entry:
 *  without one the Grant access button on the Backup page is dead (it can only
 *  toast that nothing is defined). Dropbox had no entry until 2026-08-05, which
 *  is why a Dropbox account short of `files.content.write` had no route back to
 *  working other than an agent conversation.
 *
 *  Dropbox's four: write for the folder create / upload / retention delete,
 *  read for restoring, metadata for the listing that drives pruning and the
 *  health card, and `account_info.read` so the connected account shows whose it
 *  is. The Dropbox App Console must permit each of them first; an authorize call
 *  can only narrow the console's set, never widen it. */
export const PROVIDER_SCOPES: Record<string, string> = {
  google_drive: 'https://www.googleapis.com/auth/drive.file',
  dropbox: 'files.content.write files.content.read files.metadata.read account_info.read',
};

/** The OAuth provider a backup provider connects through. The two names differ
 *  only where a Google account backs Drive; every other id is its own provider.
 *  Mirrors `oauth_provider` in the engine's `PROVIDERS` registry
 *  (`core/backup/mod.rs`). */
export function oauthProviderFor(backupProviderId: string): string {
  return backupProviderId === 'google_drive' ? 'google' : backupProviderId;
}
