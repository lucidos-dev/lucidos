/** The document's delegated click handling: thread links, inline images,
 *  copy buttons and external URLs. Only a drawn UI has anything to click, so
 *  *shell startup* installs it and its code stays out of the entry chunk. */

import { installLinkPressGuard } from '@lucidos/link-press-guard';
import { openThreadAcrossWorkspaces } from './actions/cross-workspace';
import { routeExternalAnchorClick } from './externalAnchorClick';
import { inlineMarkdownImage, openImagePopupFromGroup } from './imagePopup';
import { showToast } from './store';
import { CHECK_ICON, COPY_ICON } from '../utils/copyIcons';
import { clipboardOrReport } from '../utils/clipboard';
import { postClientLog } from '../utils/clientLog';

function onGlobalClick(e: MouseEvent) {
  const target = e.target as HTMLElement;

  // An inline markdown image opens the image popup, the only surface that
  // can zoom it. Delegated here rather than bound per component: a chat turn,
  // a rendered `.md` preview and a notification body all emit the same
  // wrapper, and none owns a click handler of its own.
  const inlineImage = inlineMarkdownImage(target);
  if (inlineImage) {
    e.preventDefault();
    openImagePopupFromGroup(inlineImage.src, inlineImage);
    return;
  }

  const threadLink = target.closest('.thread-link') as HTMLElement | null;
  if (threadLink) {
    e.preventDefault();
    const threadId = threadLink.getAttribute('data-thread-id');
    if (!threadId) return;
    // Cross-workspace links carry data-thread-workspace; the shared router
    // hops to that workspace's UI (its thread isn't in our threadMap) and
    // focuses in place otherwise.
    const linkWorkspace = threadLink.getAttribute('data-thread-workspace');
    openThreadAcrossWorkspaces(linkWorkspace ?? undefined, threadId);
    return;
  }

  // Copy buttons (copyable blocks and fenced code blocks)
  const copyBtn = target.closest('.copy-btn') as HTMLElement | null;
  if (copyBtn) {
    e.preventDefault();
    e.stopPropagation();
    // Only the renderer writes `data-copy-text` and `data-copy-code`, and
    // content cannot, so a lookalike block copies nothing
    // (utils/markdownEngine.ts).
    const text = copyBtn.closest('[data-copy-text]')?.getAttribute('data-copy-text')
      ?? copyBtn.closest('[data-copy-code]')?.querySelector('pre code')?.textContent
      ?? null;
    if (text == null) return;
    // Guarded rather than called bare: a non-secure origin exposes no
    // `navigator.clipboard`, so the unguarded call threw before a promise
    // existed and the `.catch` below never ran. See utils/clipboard.ts.
    const clipboard = clipboardOrReport();
    if (!clipboard) return;
    clipboard.writeText(text).then(() => {
      copyBtn.innerHTML = CHECK_ICON;
      copyBtn.classList.add('copy-btn-copied');
      setTimeout(() => {
        copyBtn.innerHTML = COPY_ICON;
        copyBtn.classList.remove('copy-btn-copied');
      }, 1500);
    }).catch(() => {
      showToast('Failed to copy to clipboard', 'error');
    });
    return;
  }

  // External links not handled by component-level handlers
  routeExternalAnchorClick(e);
}

/** Install the click handler and the link press guard. Returns the teardown. */
export function installGlobalClick(): () => void {
  document.addEventListener('click', onGlobalClick);
  // A swipe that starts on a link must not open it. Captured on `window`, so
  // no click listener on `document` sees the click, the wake-tap guard included.
  const stopLinkPressGuard = installLinkPressGuard(window, {
    onSwallowed: (info) => postClientLog('nav', 'link-click-after-swipe', { ...info }),
  });
  return () => {
    document.removeEventListener('click', onGlobalClick);
    stopLinkPressGuard();
  };
}
