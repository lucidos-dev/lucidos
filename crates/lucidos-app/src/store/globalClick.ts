
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
    const linkWorkspace = threadLink.getAttribute('data-thread-workspace');
    openThreadAcrossWorkspaces(linkWorkspace ?? undefined, threadId);
    return;
  }

  const copyBtn = target.closest('.copy-btn') as HTMLElement | null;
  if (copyBtn) {
    e.preventDefault();
    e.stopPropagation();
    const text = copyBtn.closest('[data-copy-text]')?.getAttribute('data-copy-text')
      ?? copyBtn.closest('[data-copy-code]')?.querySelector('pre code')?.textContent
      ?? null;
    if (text == null) return;
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

  routeExternalAnchorClick(e);
}

export function installGlobalClick(): () => void {
  document.addEventListener('click', onGlobalClick);
  const stopLinkPressGuard = installLinkPressGuard(window, {
    onSwallowed: (info) => postClientLog('nav', 'link-click-after-swipe', { ...info }),
  });
  return () => {
    document.removeEventListener('click', onGlobalClick);
    stopLinkPressGuard();
  };
}
