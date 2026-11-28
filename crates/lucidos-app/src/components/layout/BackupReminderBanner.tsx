import type { Ref, VNode } from 'preact';
import { useRef } from 'preact/hooks';
import { useCoarseClock } from '../../hooks/useCoarseClock';
import {
  backupBanner,
  dismissBackupOverdueReminder,
  dismissBackupReminder,
  type BackupBanner,
} from '../../store/actions/preferences';
import { startBackup } from '../../store/actions/backup';
import { openBackupSettings } from '../../store/actions/menu';
import { backupProgress } from '../../store/store';
import { formatAgoPhrase } from '../../utils/formatTime';
import { viewportIsMobile } from '../../utils/viewport';
import { CloseIcon } from '../shared/icons';
import { SurfaceToneIcon } from '../shared/Surface';
import { bannerBelongsToLayout, useBannerHeightVar, type BannerLayout } from './appBanner';

export const BANNER_HEIGHT_VAR = '--app-banner-height';

export function shouldRenderBanner(opts: {
  layout: BannerLayout;
  mobileViewport: boolean;
  reminderVisible: boolean;
}): boolean {
  return bannerBelongsToLayout(opts.layout, opts.mobileViewport) && opts.reminderVisible;
}

export type BackupReminder =
  | { state: 'setup'; onSetUp: () => void }
  | { state: 'overdue'; lastBackupAt: string | null; now: Date; running: boolean; onBackUp: () => void };

export function overdueText(lastBackupAt: string | null, now: Date): string {
  if (!lastBackupAt) return 'Backup is set up, but no backup has run yet.';
  return `Last backup was ${formatAgoPhrase(new Date(lastBackupAt), now)}.`;
}

function reminderControls(reminder: BackupReminder): { text: string; button: VNode } {
  if (reminder.state === 'setup') {
    return {
      text: 'Backup is off. Nothing in this workspace is being copied anywhere else.',
      button: (
        <button class="action-btn" onClick={reminder.onSetUp}>
          Set up backup
        </button>
      ),
    };
  }
  return {
    text: overdueText(reminder.lastBackupAt, reminder.now),
    button: (
      <button class="action-btn" onClick={reminder.onBackUp} disabled={reminder.running}>
        {reminder.running ? 'Backing up…' : 'Back up now'}
      </button>
    ),
  };
}

export function backupReminderBody(props: {
  layout: BannerLayout;
  reminder: BackupReminder;
  onDismiss: () => void;
  elRef?: Ref<HTMLDivElement>;
}): VNode {
  const { text, button } = reminderControls(props.reminder);
  return (
    <div ref={props.elRef} class="backup-reminder" data-layout={props.layout} role="status">
      <SurfaceToneIcon tone="warning" />
      <span class="backup-reminder-text">{text}</span>
      {button}
      <button
        class="icon-btn surface-close backup-reminder-close"
        onClick={props.onDismiss}
        aria-label="Dismiss backup reminder"
        data-tooltip="Dismiss backup reminder"
      >
        <CloseIcon />
      </button>
    </div>
  );
}

export function BackupReminderBanner({ layout }: { layout: BannerLayout }) {
  const ref = useRef<HTMLDivElement>(null);
  const now = useCoarseClock();
  const banner = backupBanner(now);
  const show = shouldRenderBanner({
    layout,
    mobileViewport: viewportIsMobile.value,
    reminderVisible: banner !== null,
  });

  useBannerHeightVar(ref, { layout, cssVar: BANNER_HEIGHT_VAR, active: show });

  if (!banner || !show) return null;

  return backupReminderBody({
    layout,
    elRef: ref,
    reminder: liveReminder(banner, now),
    onDismiss: () => {
      if (banner.state === 'overdue') void dismissBackupOverdueReminder();
      else void dismissBackupReminder();
    },
  });
}

function liveReminder(banner: BackupBanner, now: number): BackupReminder {
  if (banner.state === 'setup') return { state: 'setup', onSetUp: openBackupSettings };
  return {
    state: 'overdue',
    lastBackupAt: banner.lastBackupAt,
    now: new Date(now),
    running: backupProgress.value !== null,
    onBackUp: () => { void startBackup(banner.provider); },
  };
}
