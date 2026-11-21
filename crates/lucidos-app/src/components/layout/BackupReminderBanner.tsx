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

/** The CSS custom property this banner publishes its measured height into.
 *  `--app-header-bottom` adds it, so the toast stack, the drawer and the drawer
 *  backdrop all clear the banner instead of starting behind it. One property per
 *  banner, never a shared one: see `useBannerHeightVar`. */
export const BANNER_HEIGHT_VAR = '--app-banner-height';

/** Whether THIS instance is the one that renders: it must belong to the mounted
 *  layout and the reminder must be due. Pure so both halves are unit-testable
 *  without a DOM. */
export function shouldRenderBanner(opts: {
  layout: BannerLayout;
  mobileViewport: boolean;
  reminderVisible: boolean;
}): boolean {
  return bannerBelongsToLayout(opts.layout, opts.mobileViewport) && opts.reminderVisible;
}

/** What the bar says and offers. `setup` has no destination yet. `overdue` has
 *  one, and the last good backup (`lastBackupAt`, null for none) is too old. */
export type BackupReminder =
  | { state: 'setup'; onSetUp: () => void }
  | { state: 'overdue'; lastBackupAt: string | null; now: Date; running: boolean; onBackUp: () => void };

/** The sentence for the `overdue` state. */
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

/** Pure markup for the bar. Hook-free (the `backupHealthCard` idiom) so the
 *  tests can invoke it directly. `elRef` lands on the bar ITSELF rather than a
 *  wrapper, so the flex child the shell (and the mobile header) lays out is the
 *  same box the ResizeObserver measures. */
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

/** Sticky backup reminder: no destination yet (`setup`), or a destination whose
 *  last good backup is too old (`overdue`). `backupBannerState` picks one.
 *
 *  Persistent rather than a toast on purpose: losing a workspace is
 *  unrecoverable, and a transient corner popup is the wrong weight for a warning
 *  that stays true until acted on. Dismissing is one tap. `setup` returns after
 *  30 days and retires on a second dismissal. `overdue` returns after 7 days.
 *
 *  Both inputs are synced signals. Enabling backup retracts `setup` through the
 *  `PreferencesChanged` SSE, and a finished backup retracts `overdue` through
 *  `BackupCompleted`. The coarse clock moves what time alone changes: a verdict
 *  passing `overdue_at`, a snooze running out, and the "N days ago" label. */
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
