import { PREF_BACKUP_SCHEDULE } from '@lucidos/preference-catalog';
import type { BackupProviderInfo, BackupSchedule } from '../../api/client';
import { pickInitialProvider } from './backupProviderScopes';


export function refreshMayApply(at: {
  version: number;
  applied: number;
  writesAtStart: number;
  writesNow: number;
}): boolean {
  return at.version > at.applied && at.writesNow === at.writesAtStart;
}

export interface BackupSeed {
  providers: BackupProviderInfo[] | null;
  schedule: string | null;
  provider: string;
}

export function backupSeed(
  providersRead: PromiseSettledResult<BackupProviderInfo[]>,
  scheduleRead: PromiseSettledResult<BackupSchedule>,
  current: { provider: string; providers: BackupProviderInfo[] },
): BackupSeed {
  const providers = providersRead.status === 'fulfilled' ? providersRead.value : null;
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
    schedule: scheduleRead.value.schedule || PREF_BACKUP_SCHEDULE.fallback,
    provider: pickInitialProvider(scheduleRead.value.provider, available),
  };
}
