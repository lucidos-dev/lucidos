/**
 * What the Backup page's provider and schedule controls should show, given the
 * two reads that answer for them.
 *
 * Its own module (importing only types and `pickInitialProvider`) because the
 * decision is now made twice: once on mount, and once for every backup-relevant
 * `PreferencesChanged` that arrives over SSE. Those two paths differ in what
 * they do about a FAILURE (mount toasts and shows a failed list; a refresh the
 * user did not ask for stays silent and keeps the last known good values), and
 * they must not differ in anything else. A second, sloppier copy of the seeding
 * rules is how a provider change over SSE could end up selecting a destination
 * the mount path would have rejected, or disabling a real nightly backup.
 */
import type { BackupProviderInfo, BackupSchedule } from '../../api/client';
import { pickInitialProvider } from './backupProviderScopes';

/** May an in-flight background refresh apply what it read?
 *
 *  Two guards, and the diff that added the refresh shipped only the second one.
 *
 *  **Ordering.** One user action can bump the version twice: `PUT
 *  /backup/schedule` writes `backup_provider` AND `backup_schedule`, so the
 *  engine emits two `PreferencesChanged` events for it. The effect therefore
 *  starts a second refresh while the first is still in the air, and the two can
 *  complete in either order. Applying the older result last puts stale values on
 *  screen and, worse, records its own older version as the applied one, so the
 *  page has no reason to re-render and nothing corrects it until the next
 *  change. Only a result at least as new as what has already landed may apply.
 *
 *  **Local action.** A save or a *Grant access* started under the reads. Its
 *  outcome is newer than anything they can return, so the result is dropped and
 *  the version stays unapplied, which is what makes the effect re-read once the
 *  action finishes.
 */
export function refreshMayApply(at: {
  /** The preferences version this refresh was started for. */
  version: number;
  /** The newest version already applied. */
  applied: number;
  /** The local-action counter when this refresh started, and now. */
  writesAtStart: number;
  writesNow: number;
}): boolean {
  return at.version > at.applied && at.writesNow === at.writesAtStart;
}

/** What to apply. `null` means "this read did not answer, leave it alone". */
export interface BackupSeed {
  /** The provider registry, or null when the read failed. */
  providers: BackupProviderInfo[] | null;
  /** The configured schedule, or null when it is still UNKNOWN.
   *
   *  Unknown is not `'off'`. One endpoint writes the schedule and the provider
   *  together, and each handler sends the other half from state, so treating a
   *  failed read's default as known is how a destination change could silently
   *  disable a nightly backup. */
  schedule: string | null;
  /** Which destination to select. Always a decision: the dropdown has to show
   *  something, and on a refresh the something is usually what it already
   *  shows. */
  provider: string;
}

/** Decide from both reads TOGETHER, never from whichever settled first.
 *
 *  Seeding from the faster response is what let the registry's first entry
 *  (always Google Drive) override a real `backup_provider`, and it also made
 *  the dropdown render one provider and then flip to another.
 *
 *  `current` is what the page is showing right now: an empty provider on mount,
 *  the live selection on a refresh. It is the fallback for the one case neither
 *  read can answer, a schedule request that failed, where the configured
 *  destination is simply unknown. On mount that fallback is empty, so the page
 *  still seeds the registry's first entry rather than showing a blank dropdown,
 *  exactly as it did before this function existed. */
export function backupSeed(
  providersRead: PromiseSettledResult<BackupProviderInfo[]>,
  scheduleRead: PromiseSettledResult<BackupSchedule>,
  current: { provider: string; providers: BackupProviderInfo[] },
): BackupSeed {
  const providers = providersRead.status === 'fulfilled' ? providersRead.value : null;
  // A failed registry read still has to validate the selection against
  // something, and the registry changes between releases, not between reads.
  const available = providers ?? current.providers;
  if (scheduleRead.status !== 'fulfilled') {
    return {
      providers,
      schedule: null,
      provider: current.provider || pickInitialProvider(null, available),
    };
  }
  return {
    providers,
    schedule: scheduleRead.value.schedule || 'off',
    provider: pickInitialProvider(scheduleRead.value.provider, available),
  };
}
