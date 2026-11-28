import { packagedUpdateVersion } from './packagedUpdate';
import { owedReleaseNoticeCount } from './releaseNotices';
import { thisDeviceIsMobile } from '../utils/platform';
import type { SettingsNavKey } from './store';

function resolvableUpdateVersion(): string | null {
  return thisDeviceIsMobile() ? null : packagedUpdateVersion();
}

export function systemAttentionBadgeLabel(update: string | null, owed: number): string | null {
  const parts: string[] = [];
  if (update) parts.push(`Lucidos ${update} available`);
  if (owed > 0) parts.push(owed === 1 ? '1 thing to do' : `${owed} things to do`);
  return parts.length > 0 ? parts.join(' · ') : null;
}

export function systemAttentionBadge(): string | null {
  return systemAttentionBadgeLabel(resolvableUpdateVersion(), owedReleaseNoticeCount());
}

export function updateBadge(): string | null {
  return systemAttentionBadgeLabel(resolvableUpdateVersion(), 0);
}

export function releaseNoticeBadge(): string | null {
  return systemAttentionBadgeLabel(null, owedReleaseNoticeCount());
}

export function settingsRowBadge(key: SettingsNavKey): string | null {
  if (key === 'updates') return updateBadge();
  if (key === 'system' || key === 'release-notices') return releaseNoticeBadge();
  return null;
}
