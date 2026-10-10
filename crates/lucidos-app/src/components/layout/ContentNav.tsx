/** The content pane's navigation controls: the drawer hamburger that leads its
 *  header row, and the back / forward chevrons that bracket its title.
 *
 *  There is no group component wrapping the three. There was (`PanelNav`, which
 *  is what this file used to be called), and it stopped being expressible the
 *  moment the chevrons moved to flank the title: the hamburger leads the row
 *  and they sit in its middle, so the three are no longer adjacent on either
 *  viewport. Each header composes them itself. */
import { closeDrawer } from './Drawer';
import { drawerOpen, drawerClosing, openDrawer } from './drawerState';
import { webviewHasHistory, actionableChangeCount } from '../../store/store';
import { canGoBack, canGoForward, navBack, navForward, navHistory, navGoTo } from '../../store/actions/navigation';
import type { NavEntry } from '../../store/actions/navigation';
import { navEntryTitle, navEntryCategory } from './headerHelpers';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { isTauri } from '../../utils/platform';
import { webviewGoBack, webviewGoForward } from '../../utils/tauri';
import { MenuIcon, CloseIcon } from '../shared/icons';
import { CategoryIcon } from '../shared/CategoryIcon';
import { NavChevron, type NavHistoryItem } from '../shared/NavChevron';
import { SystemAttentionBadge } from '../shared/SystemAttentionBadge';
import { GlyphBadge } from '../shared/GlyphBadge';
import { systemAttentionBadge } from '../../store/systemAttentionBadge';

function menuDrawerShown(): boolean {
  return drawerOpen.value && !drawerClosing.value;
}

/** Open the menu drawer, or close it when it is open. `anchor` is the
 *  hamburger pressed; the shortcut has none. */
export function toggleMenuDrawer(anchor?: HTMLElement): void {
  if (menuDrawerShown()) closeDrawer();
  else openDrawer(anchor);
}

export function HamburgerButton() {
  const isOpen = menuDrawerShown();
  // The first step of the path into System, so it carries the mark, and the
  // union of both causes because it leads to both tabs. The Settings row inside
  // the menu drawer carries the next step. The mark itself is decorative, so
  // this button speaks the sentence in its own name (see
  // `SystemAttentionBadge`).
  const news = systemAttentionBadge();
  // It also leads to Changes, so it carries the drawer row's count. The count
  // takes the corner over the dot, and the label still says both.
  const changeCount = actionableChangeCount.value ?? 0;
  const changeNews = changeCount === 0 ? null
    : changeCount === 1 ? '1 change ready' : `${changeCount} changes ready`;
  const action = isOpen ? 'Close menu' : 'Open menu';
  const label = [action, changeNews, news].filter(Boolean).join(' · ');

  return (
    <button
      class={`icon-btn header-icon hamburger-panel${isOpen ? ' open' : ''}`}
      onClick={(e) => toggleMenuDrawer(e.currentTarget as HTMLElement)}
      aria-label={label}
      data-tooltip={tooltipWithShortcut(label, 'toggleMenuDrawer')}
    >
      {isOpen ? <CloseIcon /> : <MenuIcon />}
      {changeCount > 0 && (
        <GlyphBadge class="badge" aria-hidden="true">{changeCount > 99 ? '99+' : changeCount}</GlyphBadge>
      )}
      <SystemAttentionBadge placement="corner" label={changeCount > 0 ? null : news} />
    </button>
  );
}

function contentNavItem(i: number, entry: NavEntry): NavHistoryItem {
  return {
    key: String(i),
    label: navEntryTitle(entry),
    icon: <CategoryIcon category={navEntryCategory(entry)} />,
    onSelect: () => navGoTo(i),
  };
}

function contentBackItems(): NavHistoryItem[] {
  const { stack, cursor } = navHistory.value;
  const items: NavHistoryItem[] = [];
  for (let i = cursor - 1; i >= 0; i--) items.push(contentNavItem(i, stack[i]));
  return items;
}

function contentForwardItems(): NavHistoryItem[] {
  const { stack, cursor } = navHistory.value;
  const items: NavHistoryItem[] = [];
  for (let i = cursor + 1; i < stack.length; i++) items.push(contentNavItem(i, stack[i]));
  return items;
}

export function ContentBackButton() {
  const hasHistory = webviewHasHistory.value;
  // In Tauri, when the panel iframe has its own webview history the chevron
  // drives THAT (non-enumerable) stack — no app-side list to show, so the
  // long-press menu is disabled in this mode.
  const webviewMode = hasHistory && isTauri();

  return (
    <NavChevron
      direction="back"
      buttonClass="content-back-btn"
      disabled={hasHistory ? false : !canGoBack.value}
      onStep={webviewMode ? webviewGoBack : navBack}
      getItems={contentBackItems}
      ariaLabel="Back"
      tooltip={tooltipWithShortcut('Back', 'historyBack')}
      history={!webviewMode}
    />
  );
}

export function ContentForwardButton() {
  const hasHistory = webviewHasHistory.value;
  const webviewMode = hasHistory && isTauri();

  return (
    <NavChevron
      direction="forward"
      buttonClass="content-forward-btn"
      disabled={hasHistory ? false : !canGoForward.value}
      onStep={webviewMode ? webviewGoForward : navForward}
      getItems={contentForwardItems}
      ariaLabel="Forward"
      tooltip={tooltipWithShortcut('Forward', 'historyForward')}
      history={!webviewMode}
    />
  );
}
