import { drawerOpen, drawerClosing, closeDrawer, openDrawer } from './Drawer';
import { webviewHasHistory } from '../../store/store';
import { canGoBack, canGoForward, navBack, navForward } from '../../store/actions/navigation';
import { isTauri } from '../../utils/platform';
import { webviewGoBack, webviewGoForward } from '../../utils/tauri';
import { BackIcon, ForwardIcon, MenuIcon, CloseIcon } from '../shared/icons';

export function HamburgerButton() {
  const isOpen = drawerOpen.value && !drawerClosing.value;

  return (
    <button
      class={`icon-btn header-icon hamburger-panel${isOpen ? ' open' : ''}`}
      onClick={() => {
        if (isOpen) closeDrawer();
        else openDrawer();
      }}
      aria-label={isOpen ? 'Close menu' : 'Open menu'}
      data-tooltip={isOpen ? 'Close menu' : 'Open menu'}
    >
      {isOpen ? <CloseIcon /> : <MenuIcon />}
    </button>
  );
}

/** Left-side navigation group: back/forward + hamburger menu. */
export function PanelNav() {
  return (
    <div class="panel-nav">
      <HamburgerButton />
      <ContentBackButton />
      <ContentForwardButton />
    </div>
  );
}

export function ContentBackButton() {
  const hasHistory = webviewHasHistory.value;

  const goBack = hasHistory && isTauri() ? webviewGoBack : navBack;
  const backDisabled = hasHistory ? false : !canGoBack.value;

  return (
    <button
      class="icon-btn header-icon content-back-btn"
      disabled={backDisabled}
      onClick={goBack}
      aria-label="Back"
      data-tooltip="Back"
    >
      <BackIcon />
    </button>
  );
}

export function ContentForwardButton() {
  const hasHistory = webviewHasHistory.value;

  const goForward = hasHistory && isTauri() ? webviewGoForward : navForward;
  const forwardDisabled = hasHistory ? false : !canGoForward.value;

  return (
    <button
      class="icon-btn header-icon content-forward-btn"
      disabled={forwardDisabled}
      onClick={goForward}
      aria-label="Forward"
      data-tooltip="Forward"
    >
      <ForwardIcon />
    </button>
  );
}
