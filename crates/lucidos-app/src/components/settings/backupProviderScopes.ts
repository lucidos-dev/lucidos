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
 *  can only narrow the console's set, never widen it.
 *
 *  Mirrored engine-side as each provider's `GRANT_SCOPES` (`core/backup/`),
 *  which is what names an unmet requirement on the Backup page and in
 *  `get_backup_status`. Change one and change the other: a scope requested here
 *  but absent there would be reported under the engine's raw matcher instead. */
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

/** The red line shown when a provider has an account but cannot upload.
 *
 *  Names the permissions the grant is short whenever the engine reports them.
 *  The engine already resolves each unmet requirement to a scope the user can
 *  act on (`name_missing_scopes` in `core/backup/mod.rs`), so this renders what
 *  it is given: mapping again here would be a second, drifting answer to the
 *  same question, and the agent's `get_backup_status` reads the same list.
 *  Without that, the page can only repeat "access not granted", which reads the
 *  same whether the authorization never happened or came back one scope short:
 *  a user who pressed *Grant access*, completed the Dropbox consent screen and
 *  returned to the identical red line had no way to tell which had happened, or
 *  that the fix was a checkbox in the Dropbox App Console rather than another
 *  press of the button.
 *
 *  `missing` may be empty even for a not-ready provider (a verdict that could
 *  not be resolved), so the bare sentence stays as the fallback rather than
 *  rendering an empty list. It may also be genuinely ABSENT despite the type:
 *  an engine older than this field answers `/backup/providers` without it, which
 *  happens for the window between the new bundle being served and the engine
 *  restart landing. A `.length` on that would throw inside render and take the
 *  whole Settings view down, so the guard is nullish rather than a length check. */
export function backupAccessLine(providerName: string, missing: string[] | undefined): string {
  if (!missing?.length) return `${providerName} access not granted.`;
  const permissions = missing.length === 1 ? 'permission' : 'permissions';
  return `${providerName} is missing the ${missing.join(', ')} ${permissions}.`;
}

/** Which provider the Backup page should open on.
 *
 *  `configured` is the `backup_provider` preference as `GET /backup/schedule`
 *  reports it; `available` is the engine's provider registry, in registry
 *  order. The configured provider wins whenever the registry still offers it.
 *
 *  The page used to seed `available[0].id` unconditionally, which is always
 *  `google_drive`. An install configured for Dropbox therefore rendered its
 *  health card, its connected / ready verdict, its *Grant access* button and
 *  its *Back up now* button against Google Drive, and a schedule change from
 *  that state would have rewritten `backup_provider` to the provider the user
 *  never picked.
 *
 *  Falling back to the first registry entry is still right when nothing is
 *  configured: the dropdown has to show something, and the page reports the
 *  not-connected state for whatever it lands on. It is only wrong as an
 *  override of a real preference.
 *
 *  A configured id the registry does not offer (a provider retired between
 *  releases, or a hand-edited preference) also falls back, because selecting it
 *  would leave every provider-scoped control disabled with nothing on screen
 *  explaining why. */
export function pickInitialProvider(
  configured: string | null | undefined,
  available: { id: string }[],
): string {
  if (configured && available.some((p) => p.id === configured)) return configured;
  return available[0]?.id ?? '';
}
