/** The content pane's navigation controls: the drawer hamburger that leads its
 *  header row, and the back / forward chevrons that bracket its title.
 *
 *  There is no group component wrapping the three. There was (`PanelNav`, which
 *  is what this file used to be called), and it stopped being expressible the
 *  moment the chevrons moved to flank the title: the hamburger leads the row
 *  and they sit in its middle, so the three are no longer adjacent on either
 *  viewport. Each header composes them itself. */
import { drawerOpen, drawerClosing, closeDrawer, openDrawer } from './Drawer';
import { webviewHasHistory } from '../../store/store';
import { canGoBack, canGoForward, navBack, navForward, navHistory, navGoTo } from '../../store/actions/navigation';
import type { NavEntry } from '../../store/actions/navigation';
import { navEntryTitle, navEntryCategory } from './headerHelpers';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { isTauri } from '../../utils/platform';
import { webviewGoBack, webviewGoForward } from '../../utils/tauri';
import { MenuIcon, CloseIcon } from '../shared/icons';
import { CategoryIcon } from '../shared/CategoryIcon';
import { NavChevron, type NavHistoryItem } from '../shared/NavChevron';

export function HamburgerButton() {
  const isOpen = drawerOpen.value && !drawerClosing.value;

  return (
    <button
      class={`icon-btn header-icon hamburger-panel${isOpen ? ' open' : ''}`}
      onClick={(e) => {
        if (isOpen) closeDrawer();
        else openDrawer(e.currentTarget as HTMLElement);
      }}
      aria-label={isOpen ? 'Close menu' : 'Open menu'}
      data-tooltip={isOpen ? 'Close menu' : 'Open menu'}
    >
      {isOpen ? <CloseIcon /> : <MenuIcon />}
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
