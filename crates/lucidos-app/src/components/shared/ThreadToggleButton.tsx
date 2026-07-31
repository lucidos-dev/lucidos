import { toggleThreads } from '../../store/actions/pane';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { shortcutDef } from '../../utils/shortcuts';
import { attentionThreadCount, mobileView, threadDrawerOpen, type MobileView } from '../../store/store';
import { viewportIsMobile } from '../../utils/viewport';
import { ThreadsIcon } from './icons';

interface Props {
  class?: string;
}

/** Same wording as the ⌘⇧1 registry entry, derived rather than copied so a
 *  glossary-driven rename of the shortcut label can't leave this button's
 *  tooltip and aria-label on the old name while Settings shows the new one. */
const LABEL = shortcutDef('toggleThreadDrawer').label;

/** Whether the thread list is on screen right now: the threads pane on mobile
 *  (where the list is a swipe pane, not a drawer), the thread drawer on desktop.
 *  Pure so the badge rule below is testable without a DOM. */
export function threadListVisible(mobile: boolean, view: MobileView, drawerOpen: boolean): boolean {
  return mobile ? view === 'threads' : drawerOpen;
}

/** The needs-attention count the toggle should badge (0 means no badge). It
 *  rides this toggle only while the thread list is HIDDEN: with the list on
 *  screen its own header's Filter button already carries the same count, and two
 *  badges for one number read as two separate problems. Same rule on both
 *  layouts, which is what makes the mobile thread pane header (where the list is
 *  always a pane away) badge whenever anything needs the user. */
export function threadToggleBadgeCount(
  attentionCount: number, mobile: boolean, view: MobileView, drawerOpen: boolean,
): number {
  return threadListVisible(mobile, view, drawerOpen) ? 0 : attentionCount;
}

export function ThreadToggleButton({ class: cls }: Props) {
  const badgeCount = threadToggleBadgeCount(
    attentionThreadCount.value, viewportIsMobile.value, mobileView.value, threadDrawerOpen.value,
  );
  // The badge is decorative markup, so the count has to reach assistive tech
  // through the label or it's invisible there.
  const label = badgeCount > 0 ? `${LABEL} (${badgeCount} needing attention)` : LABEL;

  return (
    <button
      class={`icon-btn header-icon thread-toggle${cls ? ` ${cls}` : ''}`}
      // The toggle is purely show/hide and must not change pane focus. Its hosts
      // fire focusPane via different events: the header regions
      // (.pane-header-brand / .collapsed-thread-actions) on CLICK, and the thread
      // pane body (.pane-thread, via SplitLayout) on POINTERDOWN. Swallow BOTH so
      // toggling the drawer never shifts pane focus to the thread pane.
      // stopPropagation is bubble-phase only, so the capture-phase overlay
      // outside-dismiss still closes any open popover (see useAnchoredPopover).
      onPointerDown={(e) => e.stopPropagation()}
      onClick={(e) => { e.stopPropagation(); toggleThreads(); }}
      aria-label={label}
      data-tooltip={tooltipWithShortcut(label, 'toggleThreadDrawer')}
    >
      <ThreadsIcon />
      {badgeCount > 0 && <span class="badge">{badgeCount}</span>}
    </button>
  );
}
