import { THREAD_LIST_TOO_NARROW, toggleThreads } from '../../store/actions/pane';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { shortcutDef } from '../../utils/shortcuts';
import { blockedThreadCount, mobileView, type MobileView } from '../../store/store';
import { drawnDrawerShown, threadListCanShow } from '../layout/drawnLayout';
import { viewportIsMobile } from '../../utils/viewport';
import { SidebarIcon, ThreadListIcon } from './icons';
import { GlyphBadge } from './GlyphBadge';

interface Props {
  class?: string;
}

const LABEL = shortcutDef('toggleThreadDrawer').label;

export function threadListVisible(mobile: boolean, view: MobileView, drawerShown: boolean): boolean {
  return mobile ? view === 'threads' : drawerShown;
}

export function threadToggleBadgeCount(
  blockedCount: number, mobile: boolean, view: MobileView, drawerShown: boolean,
): number {
  return threadListVisible(mobile, view, drawerShown) ? 0 : blockedCount;
}

export function labelWithBlockedCount(label: string, blockedCount: number): string {
  return blockedCount > 0 ? `${label} (${blockedCount} blocked)` : label;
}

export function ThreadToggleButton({ class: cls }: Props) {
  const mobile = viewportIsMobile.value;
  const drawerShown = drawnDrawerShown.value;
  const badgeCount = threadToggleBadgeCount(blockedThreadCount.value, mobile, mobileView.value, drawerShown);
  const blocked = !mobile && !drawerShown && !threadListCanShow();
  const Glyph = mobile ? ThreadListIcon : SidebarIcon;
  const label = labelWithBlockedCount(LABEL, badgeCount);

  return (
    <button
      class={`icon-btn header-icon thread-toggle${cls ? ` ${cls}` : ''}`}
      onPointerDown={(e) => e.stopPropagation()}
      onClick={(e) => { e.stopPropagation(); if (!blocked) toggleThreads(); }}
      aria-label={label}
      aria-disabled={blocked || undefined}
      data-tooltip={blocked ? THREAD_LIST_TOO_NARROW : tooltipWithShortcut(label, 'toggleThreadDrawer')}
    >
      <Glyph />
      {badgeCount > 0 && <GlyphBadge class="badge">{badgeCount}</GlyphBadge>}
    </button>
  );
}
