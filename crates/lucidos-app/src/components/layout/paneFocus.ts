import { focusedPane, type FocusedPane } from '../../store/store';
import { getVisiblePromptInput } from '../chat/promptFocus';
import { isMobile } from '../../utils/viewport';
import { focusIfNeeded, isTextInput } from '../../utils/dom';

const PANE_SELECTOR: Record<FocusedPane, string> = {
  drawer: '.thread-drawer',
  thread: '.pane-thread',
  content: '.pane-content',
};

const FOCUSABLE = [
  'a[href]', 'button:not([disabled])', 'input:not([disabled])', 'select:not([disabled])',
  'textarea:not([disabled])', 'iframe', 'summary', 'audio[controls]', 'video[controls]',
  '[contenteditable]:not([contenteditable="false"])', '[tabindex]',
].map((term) => `${term}:not([tabindex="-1"])`).join(', ');

function canTakeFocus(el: HTMLElement): boolean {
  if (el.getClientRects().length === 0 || el.closest('[inert]')) return false;
  if (typeof el.checkVisibility === 'function') return el.checkVisibility({ visibilityProperty: true });
  return getComputedStyle(el).visibility !== 'hidden';
}

export function visibleFocusables(container: HTMLElement): HTMLElement[] {
  const self = container.matches(FOCUSABLE) ? [container] : [];
  return [...self, ...container.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(canTakeFocus);
}

export function paneContainer(pane: FocusedPane): HTMLElement | null {
  return document.querySelector<HTMLElement>(PANE_SELECTOR[pane]);
}

export function paneHolding(el: Element): FocusedPane | null {
  const panes = Object.keys(PANE_SELECTOR) as FocusedPane[];
  return panes.find((pane) => el.closest(PANE_SELECTOR[pane])) ?? null;
}

export function focusIntoPane(el: HTMLElement | null): void {
  if (!el) return;
  focusIfNeeded(el);
  if (document.activeElement !== el) return;
  const pane = paneHolding(el);
  if (pane) focusedPane.value = pane;
}

export const PANE_FOCUS_REGION: Record<FocusedPane, string> = {
  drawer: '.thread-drawer-list > .thread-drawer-tree',
  thread: '.thread-view .thread-content',
  content: '.content-pane-body',
};

export function shouldReconcilePaneFocus(opts: {
  mobile: boolean;
  overlayOpen: boolean;
  focusInsidePane: boolean;
}): boolean {
  return !opts.mobile && !opts.overlayOpen && !opts.focusInsidePane;
}

const RECONCILE_MAX_FRAMES = 30;

function paneScrollRegion(pane: FocusedPane, container: HTMLElement): HTMLElement | null {
  const region = container.querySelector<HTMLElement>(PANE_FOCUS_REGION[pane]);
  return region && region.getClientRects().length > 0 ? region : null;
}

function forceFallbackTarget(pane: FocusedPane, container: HTMLElement): HTMLElement | null {
  if (pane === 'thread') {
    const prompt = getVisiblePromptInput();
    if (prompt && container.contains(prompt) && prompt.getClientRects().length > 0) return prompt;
  }
  return visibleFocusables(container)[0] ?? null;
}

function focusPaneSurface(pane: FocusedPane, force: boolean): void {
  if (isMobile()) return;
  if (force) focusedPane.value = pane;
  let frames = 0;
  const attempt = (): void => {
    if (focusedPane.value !== pane) return;
    const container = paneContainer(pane);
    if (!container) return;
    const active = document.activeElement;
    if (!force) {
      const focusInsidePane = active instanceof HTMLElement && container.contains(active);
      const overlayOpen = document.documentElement.hasAttribute('data-overlay-open');
      if (!shouldReconcilePaneFocus({ mobile: false, overlayOpen, focusInsidePane })) return;
      if (isTextInput(active)) return;
    }
    const scroll = paneScrollRegion(pane, container);
    if (scroll) {
      scroll.focus({ preventScroll: true });
      if (document.activeElement === scroll) return;
    } else if (force) {
      const fallback = forceFallbackTarget(pane, container);
      if (fallback) {
        fallback.focus({ preventScroll: true });
        if (document.activeElement === fallback) return;
      }
    } else {
      return;
    }
    if (++frames < RECONCILE_MAX_FRAMES) requestAnimationFrame(attempt);
  };
  requestAnimationFrame(attempt);
}

export function reconcilePaneFocus(pane: FocusedPane): void {
  focusPaneSurface(pane, false);
}

export function focusPaneMainControl(pane: FocusedPane): void {
  focusPaneSurface(pane, true);
}

export function navigationFocusTarget<T extends { classList: { contains(c: string): boolean } }>(
  focusables: T[],
): T | undefined {
  return focusables.find((el) => !el.classList.contains('explainer-btn')) ?? focusables[0];
}

export function focusFirstFocusableWithin(el: HTMLElement): void {
  if (isMobile()) return;
  requestAnimationFrame(() => {
    navigationFocusTarget(visibleFocusables(el))?.focus({ preventScroll: true });
  });
}

export function trapTargetIndex(
  count: number,
  activeIndex: number,
  shift: boolean,
): number | null {
  if (count === 0 || activeIndex < 0) return null;
  if (shift && activeIndex === 0) return count - 1;
  if (!shift && activeIndex === count - 1) return 0;
  return null;
}

export function paneTabTarget(
  count: number,
  activeIndex: number,
  shift: boolean,
  activeInPane: boolean,
): number | null {
  if (count === 0) return null;
  if (!activeInPane) return shift ? count - 1 : 0;
  return trapTargetIndex(count, activeIndex, shift);
}

export function tabTargetWithin(
  container: HTMLElement,
  focusables: HTMLElement[],
  active: Element | null,
  shift: boolean,
): number | null {
  const activeIndex = active ? focusables.indexOf(active as HTMLElement) : -1;
  const inside = !!active && container.contains(active);
  if (inside && activeIndex < 0 && focusables.length > 0) return neighbourIndex(focusables, active!, shift);
  return paneTabTarget(focusables.length, activeIndex, shift, inside);
}

function neighbourIndex(focusables: HTMLElement[], active: Element, shift: boolean): number {
  const follows = (a: Node, b: Node) => (a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0;
  if (shift) {
    for (let i = focusables.length - 1; i >= 0; i--) if (follows(focusables[i], active)) return i;
    return focusables.length - 1;
  }
  const next = focusables.findIndex((el) => follows(active, el));
  return next >= 0 ? next : 0;
}

export function isContentPaneIframeFocus(el: Element | null): boolean {
  return !!el && el.tagName === 'IFRAME' && !!el.closest?.('.pane-content');
}

export function installPaneFocusTracking(): () => void {
  let leftByWindowBlur: Element | null = null;
  const onFocusIn = (e: FocusEvent): void => {
    const refocus = e.target === leftByWindowBlur;
    leftByWindowBlur = null;
    if (refocus || isMobile() || !(e.target instanceof Element)) return;
    const pane = paneHolding(e.target);
    if (pane) focusedPane.value = pane;
  };
  const onBlur = (): void => {
    leftByWindowBlur = document.activeElement;
    if (isMobile()) return;
    setTimeout(() => {
      if (isContentPaneIframeFocus(document.activeElement)) focusedPane.value = 'content';
    }, 0);
  };
  document.addEventListener('focusin', onFocusIn, true);
  window.addEventListener('blur', onBlur);
  return () => {
    document.removeEventListener('focusin', onFocusIn, true);
    window.removeEventListener('blur', onBlur);
  };
}

export function handlePaneTab(e: KeyboardEvent): boolean {
  if (isMobile()) return false;
  const container = paneContainer(focusedPane.value);
  if (!container) return false;
  const focusables = visibleFocusables(container);
  const target = tabTargetWithin(container, focusables, document.activeElement, e.shiftKey);
  if (target === null) return false;
  focusables[target].focus({ preventScroll: true });
  return document.activeElement === focusables[target];
}
