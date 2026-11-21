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

// These interactions keep their browser user activation, even on a cold chunk.
export function navigateToNewChat(nav: Navigation, source?: string) {
  // Single-pane overlays cover the conversation. A split keeps its app open
  // beside the draft; leaving fullscreen exposes both panes.
  if (isMobile()) {
    panelOverlay.value = null;
  } else {
    exitAppFullscreen();
  }
  // Allocate a fresh compose thread and reveal its pane before prefilling.
  unfocusThread();
  const id = ensureFocusedComposeThread();
  if (typeof nav.prompt === 'string' && nav.prompt.length > 0) {
    updateCompose(id, { text: nav.prompt });
  }
  // rAF: Preact hasn't committed the pane/unfocusThread mutations
  // yet, so a sync focus would query the pre-render DOM and miss (or hit
  // the wrong layout's) prompt-input element. Mirrors the keyboard
  // shortcut path in useKeyboardShortcuts.ts.
  requestAnimationFrame(() => focusPromptNow());
  return prefillNewChatDestination(id, nav, source);
}

export function navigateToUrl(nav: Navigation, source?: string): void {
  if (!nav.url) {
    showToast('Navigation target missing url', 'error');
    return;
  }
  // `purpose: 'oauth'` marks the one URL that has an end: an authorization
  // page the provider will redirect to the loopback callback. It always
  // goes to the OS browser, since providers refuse a sign-in flow in an
  // embedded webview. Recording it is what lets
  // `handleOAuthAccountConnected` front this window afterwards. Every other
  // URL opens wherever the user's preferences point it.
  if (nav.purpose === 'oauth') {
    openOAuthAuthorizationUrl(nav.url);
  } else {
    // `source` is carried so a browser that refuses the tab can say which
    // thread (or app) asked for it. A navigate arriving over SSE is not
    // something the user clicked, so an unattributed toast would read as
    // coming from nowhere.
    openUrl(nav.url, source);
  }
  return;
}
