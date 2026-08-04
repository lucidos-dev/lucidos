import type { Ref, VNode } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { backupReminderVisible, dismissBackupReminder } from '../../store/actions/preferences';
import { openBackupSettings } from '../../store/actions/menu';
import { viewportIsMobile } from '../../utils/viewport';
import { getRemPx } from '../../utils/dom';
import { CloseIcon } from '../shared/icons';

/** Which layout this instance belongs to. Both are mounted (the mobile one
 *  inside the fixed `.app-header`, the desktop one in the shell's flow), and each
 *  renders only under its own viewport, per the dual-render rule in
 *  `.claude/rules/frontend.md`. Rendering both would put two bars on screen and,
 *  worse, race two ResizeObservers to publish one CSS var. */
export type BannerLayout = 'desktop' | 'mobile';

/** The CSS custom property the desktop instance publishes its measured height
 *  into. `--app-header-bottom` adds it, so the toast stack, the drawer and the
 *  drawer backdrop all clear the banner instead of starting behind it. Mobile
 *  publishes nothing: its banner lives INSIDE the header element that
 *  `useHideOnScroll` already observes, so `--mobile-header-height` (and through
 *  it the mobile `--app-header-bottom` and every pane's `::before` spacer) grows
 *  on its own. */
export const BANNER_HEIGHT_VAR = '--app-banner-height';

/** Whether THIS instance is the one that renders: it must belong to the mounted
 *  layout and the reminder must be due. Pure so both halves are unit-testable
 *  without a DOM. */
export function shouldRenderBanner(opts: {
  layout: BannerLayout;
  mobileViewport: boolean;
  reminderVisible: boolean;
}): boolean {
  const mine = opts.layout === (opts.mobileViewport ? 'mobile' : 'desktop');
  return mine && opts.reminderVisible;
}

/** The value for {@link BANNER_HEIGHT_VAR}, or null to clear the property.
 *  Published in rem (mirroring `updateTitleBarHeightVar` in `useHideOnScroll.ts`)
 *  so the reservation survives a UI-scale change. */
export function bannerHeightValue(px: number | null, remSize: number): string | null {
  if (px === null || px <= 0 || remSize <= 0) return null;
  return `${px / remSize}rem`;
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

  // Desktop only: keep --app-banner-height in step with the rendered bar, which
  // can wrap to a second line on a narrow split. Clearing on teardown is what
  // stops a stale reservation from surviving a dismiss or a switch to mobile.
  useEffect(() => {
    if (layout !== 'desktop' || !show) return;
    const el = ref.current;
    if (!el) return;
    const root = document.documentElement;
    // getRemPx() is read at MEASURE time, not captured at mount. Changing the UI
    // scale rewrites --user-ui-scale, which IS the root font size (base.css
    // `html { font-size: var(--user-ui-scale, 100%) }`), so a captured value
    // goes stale exactly when the bar's pixel height changes: the observer would
    // then divide the new px by the old rem and reserve the wrong space,
    // misplacing the toast stack and drawer until the banner remounted. Same
    // reason refreshHeight() in useHideOnScroll.ts re-reads it every time.
    const publish = (px: number | null) => {
      const value = bannerHeightValue(px, getRemPx());
      if (value === null) root.style.removeProperty(BANNER_HEIGHT_VAR);
      else root.style.setProperty(BANNER_HEIGHT_VAR, value);
    };
    publish(el.getBoundingClientRect().height);
    const observer = new ResizeObserver(() => publish(el.getBoundingClientRect().height));
    observer.observe(el, { box: 'border-box' });
    return () => {
      observer.disconnect();
      publish(null);
    };
  }, [layout, show]);

  if (!show) return null;

  return backupReminderBody({
    layout,
    elRef: ref,
    onSetUp: openBackupSettings,
    onDismiss: () => { void dismissBackupReminder(); },
  });
}
