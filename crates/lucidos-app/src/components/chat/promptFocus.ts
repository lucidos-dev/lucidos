import { isElementVisible } from '../../utils/elementVisibility';
import type { RevealBarsDetail } from '../../hooks/useHideOnScroll';
import { rungFoldOpenRequest } from './controlMenuRequests';

export function activePromptInput(): HTMLElement | null {
  const el = document.activeElement as HTMLElement | null;
  return el?.dataset?.role === 'prompt-input' ? el : null;
}

export function isComposeFocusedHere(threadId: string): boolean {
  return activePromptInput()?.dataset.threadId === threadId;
}

export function getVisiblePromptInput(): HTMLElement | null {
  const els = document.querySelectorAll<HTMLElement>('[data-role="prompt-input"]');
  for (const el of els) {
    if (isElementVisible(el)) return el;
  }
  return els.length > 0 ? els[els.length - 1] : null;
}

export function openAgentMenu(): void {
  for (const button of document.querySelectorAll<HTMLElement>('.prompt-actions-row .rung-trigger')) {
    if (isElementVisible(button)) {
      button.click();
      return;
    }
  }
  const more = visibleMoreTrigger();
  if (more) rungFoldOpenRequest.value = more;
}

export function visibleMoreTrigger(): HTMLElement | null {
  for (const more of document.querySelectorAll<HTMLElement>('.prompt-actions-row [data-fold-key=""]')) {
    if (isElementVisible(more)) return more;
  }
  return null;
}

export function focusPromptNow(): void {
  const el = getVisiblePromptInput();
  if (!el) return;
  document.dispatchEvent(new CustomEvent<RevealBarsDetail>('reveal-mobile-bars', { detail: { instant: true } }));
  el.focus({ preventScroll: true });
}

export function blurPromptInputIfFocused(): void {
  activePromptInput()?.blur();
}

let actionBtnBlurInstalled = false;
export function installActionBtnBlurListener(): void {
  if (actionBtnBlurInstalled) return;
  actionBtnBlurInstalled = true;
  document.addEventListener('click', (e) => {
    const target = e.target as HTMLElement | null;
    if (!target?.closest('.action-btn')) return;
    blurPromptInputIfFocused();
  });
}
