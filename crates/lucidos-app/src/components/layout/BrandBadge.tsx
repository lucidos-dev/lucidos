import {
  updateAvailable,
  engineNewVersionReady,
  engineVersionPending,
  mobileView,
  type ApplyPhase,
} from '../../store/store';
import { notificationsMenuUnreadTotal } from '../../store/actions/app-badge';
import { liveActivityRows, type ActivityRow } from '../../store/actions/activityRows';
import { countLabel } from './unreadCountLabel';
import { HourglassIcon, ReloadIcon } from '../shared/icons';
import { GlyphBadge } from '../shared/GlyphBadge';

export type BrandBadgeState = 'busy' | 'ready' | 'pending' | 'none';

export function brandBadgeState(activityCount: number): BrandBadgeState {
  if (activityCount > 0) return 'busy';
  if (engineNewVersionReady() || updateAvailable.value) return 'ready';
  if (engineVersionPending.value) return 'pending';
  return 'none';
}

export function brandBadgeLabel(rows: ActivityRow[]): string | undefined {
  if (rows.length > 0) return rows.map((r) => r.label).join(' · ');
  const notices = brandNotices();
  if (notices.length === 0) return undefined;
  return notices.map((n) => `${n.label} ${n.value}`).join(' · ');
}

export interface BrandTooltipRow {
  label: string;
  value: string;
}

function brandNotices(): BrandTooltipRow[] {
  const notices: BrandTooltipRow[] = [];
  if (engineNewVersionReady()) notices.push({ label: 'New version', value: 'available' });
  if (updateAvailable.value) notices.push({ label: 'Client update', value: 'available' });
  if (notices.length === 0 && engineVersionPending.value) {
    notices.push({
      label: 'New code',
      value: 'pending',
    });
  }
  return notices;
}

const APPLY_TOOLTIP_LABEL: Record<ApplyPhase | 'applying', string> = {
  merging: 'Merging',
  'resolving-conflict': 'Merge conflicts',
  hardening: 'Hardening',
  applying: 'Applying',
};

export function brandTooltipRows(
  unreadCount: number,
  rows: ActivityRow[],
  connection: { title: string; detail: string } | null,
): BrandTooltipRow[] {
  const out: BrandTooltipRow[] = [];
  if (connection) out.push({ label: connection.title, value: connection.detail });
  if (unreadCount > 0) out.push({ label: 'Unread', value: String(unreadCount) });
  const threadsPerPhase = new Map<string, number>();
  for (const row of rows) {
    if (row.body.kind === 'apply-thread') {
      const label = APPLY_TOOLTIP_LABEL[row.body.thread.reading?.phase ?? 'applying'];
      threadsPerPhase.set(label, (threadsPerPhase.get(label) ?? 0) + 1);
    } else {
      out.push({ label: row.label, value: row.detail ?? '' });
    }
  }
  for (const [label, count] of threadsPerPhase) out.push({ label, value: String(count) });
  if (rows.length === 0) out.push(...brandNotices());
  return out;
}

export function unreadBadgeLabel(count: number): string | null {
  if (count <= 0) return null;
  return count === 1 ? '1 unread notification' : `${count} unread notifications`;
}

export function UnreadBrandBadge() {
  const count = notificationsMenuUnreadTotal.value;
  if (count <= 0) return null;
  return (
    <GlyphBadge class="badge brand-unread-badge" aria-hidden="true">
      {countLabel(count)}
    </GlyphBadge>
  );
}

export function BrandBadge() {
  const rows = liveActivityRows();
  const state = brandBadgeState(rows.length);
  if (state === 'none') return null;
  if (state === 'ready') {
    return <GlyphBadge class="badge brand-badge" aria-hidden="true">!</GlyphBadge>;
  }
  if (state === 'pending') {
    return <span class="badge brand-badge brand-badge-dot" aria-hidden="true" />;
  }
  const waiting = rows.every((r) => r.queued);
  return (
    <span
      class="badge brand-badge badge-icon"
      aria-hidden="true"
    >
      {!waiting && <span key={mobileView.value} class="brand-badge-spinner"><ReloadIcon /></span>}
      {waiting && <span class="brand-badge-queued"><HourglassIcon /></span>}
    </span>
  );
}
