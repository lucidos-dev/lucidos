import type { Ref, VNode } from 'preact';
import { useRef } from 'preact/hooks';
import { backupReminderVisible, dismissBackupReminder } from '../../store/actions/preferences';
import { openBackupSettings } from '../../store/actions/menu';
import { viewportIsMobile } from '../../utils/viewport';
import { CloseIcon } from '../shared/icons';
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

/** Pure markup for the bar. Hook-free (the `backupHealthCard` idiom) so the
 *  tests can invoke it directly. `elRef` lands on the bar ITSELF rather than a
 *  wrapper, so the flex child the shell (and the mobile header) lays out is the
 *  same box the ResizeObserver measures. */
export function backupReminderBody(props: {
  layout: BannerLayout;
  onSetUp: () => void;
  onDismiss: () => void;
  elRef?: Ref<HTMLDivElement>;
}): VNode {
  return (
    <div ref={props.elRef} class="backup-reminder" data-layout={props.layout} role="status">
      <span class="backup-reminder-text">
        Backup is off. Nothing in this workspace is being copied anywhere else.
      </span>
      <button class="action-btn action-btn-confirm" onClick={props.onSetUp}>
        Set up backup
      </button>
      <button
        class="icon-btn backup-reminder-close"
        onClick={props.onDismiss}
        aria-label="Dismiss backup reminder"
      >
        <CloseIcon />
      </button>
    </div>
  );
}

/** Sticky reminder that this workspace has no automatic backup configured.
 *
 *  Persistent rather than a toast on purpose: losing a workspace is
 *  unrecoverable, and a transient corner popup is the wrong weight for a warning
 *  that stays true until acted on. Dismissing is still one tap, and a second tap
 *  (once the 30-day snooze lapses) retires it for good.
 *
 *  Visibility is a pure read of the already-synced preference map, so enabling
 *  backup in Settings retracts this live on every device via the existing
 *  `PreferencesChanged` SSE. */
export function BackupReminderBanner({ layout }: { layout: BannerLayout }) {
  const ref = useRef<HTMLDivElement>(null);
  const show = shouldRenderBanner({
    layout,
    mobileViewport: viewportIsMobile.value,
    reminderVisible: backupReminderVisible(),
  });

  useBannerHeightVar(ref, { layout, cssVar: BANNER_HEIGHT_VAR, active: show });

  if (!show) return null;

  return backupReminderBody({
    layout,
    elRef: ref,
    onSetUp: openBackupSettings,
    onDismiss: () => { void dismissBackupReminder(); },
  });
}
