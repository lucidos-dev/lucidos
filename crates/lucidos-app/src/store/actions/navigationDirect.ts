import { exitAppFullscreen } from './apps';
import { openUrl } from './artifacts';
import { openOAuthAuthorizationUrl } from './oauth';
import { unfocusThread } from './threads';
import { ensureFocusedComposeThread, updateCompose } from './compose';
import { prefillNewChatDestination } from './newChatPrefill';
import { focusPromptNow } from '../../components/chat/promptFocus';
import { isMobile } from '../../utils/viewport';
import { panelOverlay, showToast } from '../store';
type Navigation = Parameters<typeof import('./navigation-request')['handleNavigationRequest']>[0];

export function navigateToNewChat(nav: Navigation, source?: string) {
  if (isMobile()) {
    panelOverlay.value = null;
  } else {
    exitAppFullscreen();
  }
  unfocusThread();
  const id = ensureFocusedComposeThread();
  if (typeof nav.prompt === 'string' && nav.prompt.length > 0) {
    updateCompose(id, { text: nav.prompt });
  }
  requestAnimationFrame(() => focusPromptNow());
  return prefillNewChatDestination(id, nav, source);
}

export function navigateToUrl(nav: Navigation, source?: string): void {
  if (!nav.url) {
    showToast('Navigation target missing url', 'error');
    return;
  }
  if (nav.purpose === 'oauth') {
    openOAuthAuthorizationUrl(nav.url);
  } else {
    openUrl(nav.url, source);
  }
  return;
}
