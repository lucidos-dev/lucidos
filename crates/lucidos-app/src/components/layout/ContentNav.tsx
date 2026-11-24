import { closeDrawer, drawerOpen, drawerClosing, openDrawer } from './drawerState';
import { webviewHasHistory, actionableChangeCount } from '../../store/store';
import { canGoBack, canGoForward, navBack, navForward, navHistory, navGoTo } from '../../store/actions/navigation';
import type { NavEntry } from '../../store/actions/navigation';
import { navEntryTitle, navEntryCategory } from './headerHelpers';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { isTauri } from '../../utils/platform';
import { webviewGoBack, webviewGoForward } from '../../utils/tauri';
import { MenuIcon, CloseIcon } from '../shared/icons';
import { CategoryIcon } from '../shared/CategoryIcon';
import { AppIcon } from '../shared/AppIcon';
import { NavChevron, type NavHistoryItem } from '../shared/NavChevron';
import { SystemAttentionBadge } from '../shared/SystemAttentionBadge';
import { GlyphBadge } from '../shared/GlyphBadge';
import { systemAttentionBadge } from '../../store/systemAttentionBadge';

function menuDrawerShown(): boolean {
  return drawerOpen.value && !drawerClosing.value;
}

export function toggleMenuDrawer(anchor?: HTMLElement): void {
  if (menuDrawerShown()) closeDrawer();
  else openDrawer(anchor);
}

export function HamburgerButton() {
  const isOpen = menuDrawerShown();
  const news = systemAttentionBadge();
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
    icon: entry.overlay?.type === 'app-ui'
      ? <AppIcon appId={entry.overlay.app.id} name={entry.overlay.app.name} icon={entry.overlay.app.icon} />
      : <CategoryIcon category={navEntryCategory(entry)} />,
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
