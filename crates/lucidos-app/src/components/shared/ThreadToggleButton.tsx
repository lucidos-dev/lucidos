import { toggleThreads } from '../../store/actions/pane';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { shortcutDef } from '../../utils/shortcuts';
import { blockedThreadCount, mobileView, threadDrawerOpen, type MobileView } from '../../store/store';
import { viewportIsMobile } from '../../utils/viewport';
import { SidebarIcon, ThreadListIcon } from './icons';
import { GlyphBadge } from './GlyphBadge';

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

/** The Blocked count the toggle should badge (0 means no badge). It
 *  rides this toggle only while the thread list is HIDDEN. With the list on
 *  screen, the grouping button (under Folders) or the Blocked group
 *  (under Ongoing) already carries it. Two badges for one number read as two
 *  separate problems. Same rule on both
 *  layouts, which is what makes the mobile thread pane header (where the list is
 *  always a pane away) badge whenever anything needs the user. */
export function threadToggleBadgeCount(
  blockedCount: number, mobile: boolean, view: MobileView, drawerOpen: boolean,
): number {
  return threadListVisible(mobile, view, drawerOpen) ? 0 : blockedCount;
}

/** `label` with the Blocked count a badge beside it draws. The badge is
 *  decorative markup, so the count reaches assistive tech only this way. */
export function labelWithBlockedCount(label: string, blockedCount: number): string {
  return blockedCount > 0 ? `${label} (${blockedCount} blocked)` : label;
}

export function ThreadToggleButton({ class: cls }: Props) {
  const mobile = viewportIsMobile.value;
  const badgeCount = threadToggleBadgeCount(
    blockedThreadCount.value, mobile, mobileView.value, threadDrawerOpen.value,
  );
  // A sidebar glyph where the list IS a sidebar. On a phone the toggle swipes
  // to a whole pane of threads, so it draws the list it takes you to.
  const Glyph = mobile ? ThreadListIcon : SidebarIcon;
  const label = labelWithBlockedCount(LABEL, badgeCount);

  return (
    <button
      class={`icon-btn header-icon thread-toggle${cls ? ` ${cls}` : ''}`}
      // The toggle is purely show/hide and must not change pane focus, whatever
      // it is mounted inside. Header regions focus their pane on CLICK and pane
      // bodies on POINTERDOWN (SplitLayout), so swallow BOTH.
      // stopPropagation is bubble-phase only, so the capture-phase overlay
      // outside-dismiss still closes any open popover (see useAnchoredPopover).
      onPointerDown={(e) => e.stopPropagation()}
      onClick={(e) => { e.stopPropagation(); toggleThreads(); }}
      aria-label={label}
      data-tooltip={tooltipWithShortcut(label, 'toggleThreadDrawer')}
    >
      <Glyph />
      {badgeCount > 0 && <GlyphBadge class="badge">{badgeCount}</GlyphBadge>}
    </button>
  );
}
