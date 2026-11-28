import { useEffect } from 'preact/hooks';
import { composeViewActive, mobileView, preferences } from '../store/store';
import { opensSoftwareKeyboard, getRemPx } from '../utils/dom';
import { isOverlayField } from '../utils/softwareKeyboard';
import { afterPressSettles } from '../utils/pointerPress';
import {
  holdAcrossRelayout,
  isHeaderPinnedForScroll,
  onRebasedScroll,
  type NavScrollKind,
  type ScrollClass,
} from '../components/chat/scrollState';
import { onClassifiedScroll } from '../components/chat/classifiedScroll';
import { viewportIsMobile } from '../utils/viewport';
import { isRepaintNudging } from '../utils/webkitRepaint';
import { isUserScrolling } from '../utils/scrollActivity';
import { currentMobileDynamicBars } from '../store/actions/preferences';
import { findSurface } from '../store/actions/find-bar';
import {
  parseAppScrollMessage, parseAppTouchMessage, type AppScrollMessage, type AppTouchMessage,
} from '../store/actions/app-scroll-bridge';
import { APP_PANEL_SELECTOR } from '../store/appFullscreenHost';
import { CANVAS_APP_FRAME_SELECTOR } from '../utils/appFrame';

export function shouldKeepHeaderVisible(opts: { dynamicBars: boolean; findOpen: boolean }): boolean {
  return !opts.dynamicBars || opts.findOpen;
}

export function spacerHeightPx(opts: {
  cachedHeight: number;
  controlsTop: number;
  keyboardOpen: boolean;
  disabled: boolean;
}): number {
  return opts.keyboardOpen && !opts.disabled ? opts.controlsTop : opts.cachedHeight;
}

export function countsAsReaderScroll(opts: {
  navigation: boolean;
  headerPinned: boolean;
}): boolean {
  return !opts.headerPinned && !opts.navigation;
}

export function revealsBars(opts: { kind: NavScrollKind; moved: boolean }): boolean {
  return opts.moved && (opts.kind === 'placement' || opts.kind === 'held');
}

export interface RevealBarsDetail {
  instant: boolean;
}

export function landWithoutGlide(el: HTMLElement) {
  el.style.transition = 'none';
  void getComputedStyle(el).translate;
  el.style.transition = '';
}

export const BARS_TRAVEL_REM = 0.75;

export interface BarsIntent {
  away: boolean;
  travel: number;
}

export const BARS_SHOWN: BarsIntent = { away: false, travel: 0 };

export function nextBarsIntent(intent: BarsIntent, delta: number, thresholdPx: number): BarsIntent {
  if (delta === 0) return intent;
  const sameWay = Math.sign(delta) === Math.sign(intent.travel);
  const travel = sameWay ? intent.travel + delta : delta;
  if (travel >= thresholdPx) return { away: true, travel };
  if (travel <= -thresholdPx) return { away: false, travel };
  return { away: intent.away, travel };
}

export function headerOffsetPx(opts: { away: boolean; scrollTop: number; chromeHeight: number }): number {
  return opts.away && opts.scrollTop > opts.chromeHeight ? -opts.chromeHeight : 0;
}

export function chromeOffsetPx(opts: {
  disabled: boolean;
  keyboardOpen: boolean;
  away: boolean;
  scrollTop: number;
  headerHeight: number;
  titleBarHeight: number;
}): number {
  if (opts.disabled) return 0;
  const chromeHeight = opts.headerHeight + opts.titleBarHeight;
  if (opts.keyboardOpen) return -chromeHeight;
  return headerOffsetPx({ away: opts.away, scrollTop: opts.scrollTop, chromeHeight });
}

export function titleBarOffsetPx(opts: {
  onOwnPane: boolean;
  chromeOffset: number;
  paneScrollTop: number;
  headerHeight: number;
  titleBarHeight: number;
}): number {
  if (opts.onOwnPane) return opts.chromeOffset;
  const away = opts.chromeOffset < 0 && opts.paneScrollTop > opts.titleBarHeight;
  return away ? -(opts.headerHeight + opts.titleBarHeight) : 0;
}

export function promptOffsetPx(opts: {
  onThreadPane: boolean;
  away: boolean;
  chromeOffset: number;
  promptHeight: number;
  distanceToBottom: number;
}): number {
  const away = opts.onThreadPane ? opts.away : opts.chromeOffset < 0;
  const atEdge = opts.distanceToBottom <= opts.promptHeight;
  return away && !atEdge ? opts.promptHeight : 0;
}

export function promptCanSlide(opts: {
  disabled: boolean;
  keyboardOpen: boolean;
  composeEmpty: boolean;
}): boolean {
  return !opts.disabled && !opts.keyboardOpen && !opts.composeEmpty;
}

const SCROLL_SELECTORS: Record<string, string> = {
  threads: '.mobile-swipe-pane .thread-drawer-list',
  thread: '.mobile-swipe-pane .thread-content.visible',
  content: '.mobile-swipe-pane .content-pane-body',
};

export const APP_PANEL_BARS_AWAY_ATTR = 'data-bars-away';

export function useHideOnScroll(headerRef: { current: HTMLElement | null }) {
  const mobileLayout = viewportIsMobile.value;
  useEffect(() => {
    if (!mobileLayout) return;

    let prevScrollTop = 0;
    let intent = BARS_SHOWN;
    let viewScrollTop = 0;
    let distanceToBottomPx = Infinity;
    let cachedHeight = 0;
    let cachedControlsTop = 0;
    let titleBarHeight = 0;
    let titleBarEl: HTMLElement | null = null;
    let threadScroller: Element | null = null;
    let titleBarFresh = false;
    let chevronEl: HTMLElement | null = null;
    let titleBarResizeObserver: ResizeObserver | null = null;
    let currentContainer: Element | null = null;
    let stopContainerScroll: (() => void) | null = null;
    let currentContainerPane: Element | null = null;
    let currentViewKey: string | null = null;
    let mutationRafId: number | null = null;
    let rebaseSettleRaf: number | null = null;
    let rebasedTop = -1;
    let keyboardOpen = false;
    let disposed = false;
    let closePending = false;
    let disabled = false;
    let dynamicBars = false;
    let cachedRemSize = getRemPx();
    let lastOffsetRem = 0;
    let lastTitleOffsetRem = 0;
    let promptEl: HTMLElement | null = null;
    let downChevronEl: HTMLElement | null = null;
    let promptResizeObserver: ResizeObserver | null = null;
    let promptHeight = 0;
    let lastPromptOffsetRem = 0;
    let appPanelEl: HTMLElement | null = null;
    let appFrameEl: HTMLIFrameElement | null = null;
    let appScroll: { top: number; max: number } | null = null;
    let lastAppPanelAway = false;
    let appTouching = false;
    let heldOffset = 0;
    let lastHeaderTranslate = '';

    function revealBars() {
      intent = BARS_SHOWN;
      appTouching = false;
    }

    function measureContainer(container: Element) {
      if (appPanelEl && container.contains(appPanelEl)) {
        viewScrollTop = appScroll ? Math.min(Math.max(0, appScroll.top), appScroll.max) : 0;
        distanceToBottomPx = appScroll ? appScroll.max - viewScrollTop : Infinity;
        return;
      }
      viewScrollTop = clampedScrollTop(container);
      distanceToBottomPx = distanceToBottom(container, viewScrollTop);
    }

    function distanceToBottom(container: Element, scrollTop = clampedScrollTop(container)): number {
      return Math.max(0, container.scrollHeight - container.clientHeight) - scrollTop;
    }

    function updateHeaderVar() {
      const heightPx = spacerHeightPx({
        cachedHeight,
        controlsTop: cachedControlsTop,
        keyboardOpen,
        disabled,
      });
      const heightRem = heightPx / cachedRemSize;
      document.documentElement.style.setProperty('--mobile-header-height', `${heightRem}rem`);
    }

    function updateTitleBarHeightVar() {
      syncRemSize();
      const newHeight = titleBarEl ? titleBarEl.getBoundingClientRect().height : 0;
      if (Math.abs(newHeight - titleBarHeight) <= 0.1) return;
      titleBarHeight = newHeight;
      publishTitleBarHeightVar();
    }

    function publishTitleBarHeightVar() {
      if (titleBarHeight > 0) {
        document.documentElement.style.setProperty('--mobile-thread-title-height', `${titleBarHeight / cachedRemSize}rem`);
      } else {
        document.documentElement.style.removeProperty('--mobile-thread-title-height');
      }
    }

    function syncRemSize() {
      const remPx = getRemPx();
      if (remPx === cachedRemSize) return;
      cachedRemSize = remPx;
      updateHeaderVar();
      titleBarHeight = titleBarEl ? titleBarEl.getBoundingClientRect().height : 0;
      publishTitleBarHeightVar();
      promptHeight = promptEl ? promptEl.getBoundingClientRect().height : 0;
      updatePromptHeightVar();
    }

    function bindTitleBar(el: HTMLElement | null) {
      if (el === titleBarEl) return;
      if (titleBarResizeObserver) {
        titleBarResizeObserver.disconnect();
        titleBarResizeObserver = null;
      }
      titleBarEl = el;
      titleBarFresh = el !== null;
      lastTitleOffsetRem = NaN;
      updateTitleBarHeightVar();
      if (el) {
        titleBarResizeObserver = new ResizeObserver(() => {
          updateTitleBarHeightVar();
          applyTransform();
        });
        titleBarResizeObserver.observe(el);
      }
    }

    function bindOffsetConsumers(container: Element | null) {
      threadScroller = document.querySelector(SCROLL_SELECTORS.thread);
      const nextTitleBar = document.querySelector<HTMLElement>(`${SCROLL_SELECTORS.thread} .mobile-thread-title-row`);
      const nextChevron = (container?.parentElement?.querySelector(':scope > .scroll-to-top') ?? null) as HTMLElement | null;
      const nextDownChevron = (threadScroller?.parentElement?.querySelector(':scope > .scroll-to-bottom') ?? null) as HTMLElement | null;
      if (nextTitleBar === titleBarEl && nextChevron === chevronEl && nextDownChevron === downChevronEl) return false;
      bindTitleBar(nextTitleBar);
      chevronEl = nextChevron;
      downChevronEl = nextDownChevron;
      lastOffsetRem = NaN;
      lastPromptOffsetRem = NaN;
      return true;
    }

    function bindAppPanel(): boolean {
      const panel = document.querySelector<HTMLElement>(`.mobile-swipe-pane ${APP_PANEL_SELECTOR}`);
      const frame = document.querySelector<HTMLIFrameElement>(`.mobile-swipe-pane ${CANVAS_APP_FRAME_SELECTOR}`);
      if (panel === appPanelEl && frame === appFrameEl) return false;
      if (appPanelEl !== panel) {
        appPanelEl?.removeAttribute(APP_PANEL_BARS_AWAY_ATTR);
        lastAppPanelAway = false;
      }
      appPanelEl = panel;
      appFrameEl = frame;
      appScroll = null;
      return true;
    }

    function updatePromptHeightVar() {
      const root = document.documentElement.style;
      if (dynamicBars && promptHeight > 0) {
        root.setProperty('--mobile-prompt-height', `${promptHeight / cachedRemSize}rem`);
      } else {
        root.removeProperty('--mobile-prompt-height');
      }
    }

    function refreshPromptHeight() {
      syncRemSize();
      promptHeight = promptEl ? promptEl.getBoundingClientRect().height : 0;
      updatePromptHeightVar();
      applyTransform();
    }

    function clearPromptOffset() {
      if (!promptEl) return;
      promptEl.style.translate = '';
      promptEl.style.removeProperty('--mobile-prompt-offset');
    }

    function bindPrompt() {
      const next = document.querySelector<HTMLElement>('.mobile-swipe-pane .prompt-area');
      if (next === promptEl) return;
      promptResizeObserver?.disconnect();
      promptResizeObserver = null;
      clearPromptOffset();
      promptEl = next;
      lastPromptOffsetRem = NaN;
      if (next) {
        promptResizeObserver = new ResizeObserver(refreshPromptHeight);
        promptResizeObserver.observe(next, { box: 'border-box' });
      }
      refreshPromptHeight();
    }

    function applyTransform() {
      const onThreadPane = currentViewKey === 'thread';
      const offset = appTouching ? heldOffset : headerRef.current ? chromeOffsetPx({
        disabled,
        keyboardOpen,
        away: intent.away,
        scrollTop: viewScrollTop,
        headerHeight: cachedHeight,
        titleBarHeight: onThreadPane ? titleBarHeight : 0,
      }) : 0;
      heldOffset = offset;
      let threadScrollTop = viewScrollTop;
      let threadDistanceToBottom = distanceToBottomPx;
      if (!onThreadPane && offset < 0) {
        threadScrollTop = threadScroller ? clampedScrollTop(threadScroller) : 0;
        threadDistanceToBottom = threadScroller ? distanceToBottom(threadScroller, threadScrollTop) : 0;
      }
      if (headerRef.current) {
        const titleOffsetRem = titleBarOffsetPx({
          onOwnPane: onThreadPane,
          chromeOffset: offset,
          paneScrollTop: threadScrollTop,
          headerHeight: cachedHeight,
          titleBarHeight,
        }) / cachedRemSize;
        const headerOffset = Math.max(-cachedHeight, offset);
        const headerTranslate = headerOffset !== 0 ? `0 ${headerOffset / cachedRemSize}rem` : '';
        if (headerTranslate !== lastHeaderTranslate) {
          lastHeaderTranslate = headerTranslate;
          headerRef.current.style.translate = headerTranslate;
        }
        const offsetRem = offset / cachedRemSize;
        if (offsetRem !== lastOffsetRem) {
          lastOffsetRem = offsetRem;
          chevronEl?.style.setProperty('--mobile-header-offset', `${offsetRem}rem`);
        }
        if (titleOffsetRem !== lastTitleOffsetRem) {
          lastTitleOffsetRem = titleOffsetRem;
          titleBarEl?.style.setProperty('--mobile-header-offset', `${titleOffsetRem}rem`);
          if (titleBarEl && titleBarFresh) landWithoutGlide(titleBarEl);
          titleBarFresh = false;
        }
      }
      const slides = promptCanSlide({ disabled, keyboardOpen, composeEmpty: composeViewActive.peek() });
      const promptOffset = slides ? promptOffsetPx({
        onThreadPane,
        away: intent.away,
        chromeOffset: offset,
        promptHeight,
        distanceToBottom: threadDistanceToBottom,
      }) : 0;
      const promptOffsetRem = promptOffset / cachedRemSize;
      if (promptOffsetRem !== lastPromptOffsetRem) {
        lastPromptOffsetRem = promptOffsetRem;
        if (promptEl) {
          promptEl.style.translate = promptOffsetRem ? `0 ${promptOffsetRem}rem` : '';
          promptEl.style.setProperty('--mobile-prompt-offset', `${promptOffsetRem}rem`);
        }
        downChevronEl?.style.setProperty('--mobile-prompt-offset', `${promptOffsetRem}rem`);
      }
      const appPanelAway = offset < 0 && !keyboardOpen;
      if (appPanelEl && appPanelAway !== lastAppPanelAway) {
        lastAppPanelAway = appPanelAway;
        appPanelEl.toggleAttribute(APP_PANEL_BARS_AWAY_ATTR, appPanelAway);
      }
    }

    function syncToScroll(container: Element | null) {
      if (container) {
        measureContainer(container);
        prevScrollTop = viewScrollTop;
      } else {
        prevScrollTop = 0;
      }
      revealBars();
      applyTransform();
    }

    function refreshHeight() {
      syncRemSize();
      const el = headerRef.current;
      if (!el) {
        if (cachedHeight !== 0 || cachedControlsTop !== 0) {
          cachedHeight = 0;
          cachedControlsTop = 0;
          updateHeaderVar();
        }
        return;
      }
      const h = el.getBoundingClientRect().height;
      if (Math.abs(h - cachedHeight) <= 0.1) return;
      cachedHeight = h;
      cachedControlsTop = parseFloat(getComputedStyle(el).paddingTop) || 0;
      updateHeaderVar();
      applyTransform();
    }

    function clampedScrollTop(container: Element): number {
      const maxScroll = Math.max(0, container.scrollHeight - container.clientHeight);
      return Math.min(Math.max(0, container.scrollTop), maxScroll);
    }

    function onScroll(scroll: ScrollClass) {
      const header = headerRef.current;
      if (!header || cachedHeight === 0 || !currentContainer || disabled) return;

      recoverKeyboardState();

      if (isRepaintNudging() && !isUserScrolling()) return;

      const scrollTop = clampedScrollTop(currentContainer);

      const atRebasedTop = rebasedTop >= 0 && Math.abs(scrollTop - rebasedTop) <= 1;
      if (!atRebasedTop) rebasedTop = -1;

      const readerScroll = countsAsReaderScroll({
        navigation: scroll.page.navigation,
        headerPinned: isHeaderPinnedForScroll(),
      });
      if (!readerScroll) {
        prevScrollTop = scrollTop;
        measureContainer(currentContainer);
        if (scroll.page.kind !== 'anchor' && scroll.page.kind !== 'carry' && !atRebasedTop) revealBars();
        applyTransform();
        return;
      }

      const active = document.activeElement;
      if (active && opensSoftwareKeyboard(active)) {
        const activePane = active.closest('.mobile-swipe-pane');
        if (activePane === currentContainerPane) return;
      }

      intent = nextBarsIntent(intent, scrollTop - prevScrollTop, BARS_TRAVEL_REM * cachedRemSize);
      prevScrollTop = scrollTop;
      measureContainer(currentContainer);
      applyTransform();
    }

    function onAppTouch(touch: AppTouchMessage) {
      const next = touch.touching && currentViewKey === 'content' && !disabled;
      if (next === appTouching) return;
      appTouching = next;
      if (!next) applyTransform();
    }

    function onAppScroll(report: AppScrollMessage) {
      appScroll = { top: report.top, max: report.max };
      if (!headerRef.current || cachedHeight === 0 || !currentContainer || disabled) return;
      if (currentViewKey !== 'content') return;
      recoverKeyboardState();
      intent = nextBarsIntent(intent, report.delta, BARS_TRAVEL_REM * cachedRemSize);
      measureContainer(currentContainer);
      applyTransform();
    }

    function onAppMessage(e: MessageEvent) {
      const message = parseAppTouchMessage(e) ?? parseAppScrollMessage(e);
      if (!message || !appFrameEl || message.source !== appFrameEl.contentWindow) return;
      if ('touching' in message) onAppTouch(message);
      else onAppScroll(message);
    }

    function recoverKeyboardState() {
      if (!keyboardOpen || closePending) return;
      const active = document.activeElement;
      if (active && opensSoftwareKeyboard(active)) return;
      keyboardOpen = false;
      updateHeaderVar();
      syncToScroll(currentContainer);
    }

    function attachListener() {
      const view = mobileView.value;
      const selector = SCROLL_SELECTORS[view];
      if (!selector) return;

      const container = document.querySelector(selector);

      recoverKeyboardState();

      if (container !== currentContainer) {
        revealBars();
        applyTransform();
      }

      const rebound = bindOffsetConsumers(container);
      bindPrompt();
      const appRebound = bindAppPanel();
      if (appRebound) revealBars();

      if (container === currentContainer) {
        if (rebound || appRebound) {
          if (appRebound && container) measureContainer(container);
          applyTransform();
        }
        return;
      }

      stopContainerScroll?.();
      stopContainerScroll = null;

      currentContainer = container;
      currentContainerPane = container?.closest('.mobile-swipe-pane') ?? null;
      currentViewKey = view;
      rebasedTop = -1;
      refreshHeight();
      if (container) {
        measureContainer(container);
        prevScrollTop = viewScrollTop;
        stopContainerScroll = onClassifiedScroll(container as HTMLElement, onScroll);
      } else {
        prevScrollTop = 0;
        viewScrollTop = 0;
        distanceToBottomPx = Infinity;
      }
      applyTransform();
    }

    refreshHeight();
    attachListener();
    window.addEventListener('message', onAppMessage);

    function onFocusIn(e: FocusEvent) {
      if (!opensSoftwareKeyboard(e.target)) return;
      const target = e.target as HTMLElement;
      if (headerRef.current?.contains(target)) return;
      if (target.closest('.mobile-thread-title-row')) return;
      if (isOverlayField(target)) return;
      if (disabled) return;
      if (keyboardOpen) return;
      keyboardOpen = true;
      updateHeaderVar();
      if (currentContainer) {
        const delta = cachedHeight - cachedControlsTop;
        holdAcrossRelayout(currentContainer as HTMLElement, Math.max(0, currentContainer.scrollTop - delta));
      }
      applyTransform();
    }
    document.addEventListener('focusin', onFocusIn);

    function onFocusOut(e: FocusEvent) {
      if (!opensSoftwareKeyboard(e.target)) return;
      if (headerRef.current?.contains(e.target as HTMLElement)) return;
      if (!keyboardOpen) return;
      if (keepsHeaderAway(e.relatedTarget as Element | null)) return;
      closePending = true;
      afterPressSettles(() => {
        closePending = false;
        if (disposed || !keyboardOpen || keepsHeaderAway(document.activeElement)) return;
        keyboardOpen = false;
        updateHeaderVar();
        if (currentContainer) {
          holdAcrossRelayout(currentContainer as HTMLElement, currentContainer.scrollTop + cachedHeight - cachedControlsTop);
        }
        syncToScroll(currentContainer);
      });
    }
    function keepsHeaderAway(el: Element | null): boolean {
      return !!el && opensSoftwareKeyboard(el) && !headerRef.current?.contains(el)
        && !el.closest('.mobile-thread-title-row');
    }
    document.addEventListener('focusout', onFocusOut);

    const observer = new MutationObserver(() => {
      if (mutationRafId !== null) return;
      mutationRafId = requestAnimationFrame(() => {
        mutationRafId = null;
        attachListener();
        refreshHeight();
        if (currentContainer && cachedHeight > 0 && !keyboardOpen && !disabled) {
          measureContainer(currentContainer);
          applyTransform();
        }
      });
    });
    const swipeWrapper = document.querySelector('.mobile-swipe-wrapper');
    if (swipeWrapper) {
      observer.observe(swipeWrapper, { childList: true, subtree: true });
    }

    const headerResizeObserver = new ResizeObserver(() => {
      refreshHeight();
    });
    if (headerRef.current) headerResizeObserver.observe(headerRef.current, { box: 'border-box' });

    window.addEventListener('resize', refreshHeight);
    window.visualViewport?.addEventListener('resize', refreshHeight);
    function onResume() {
      if (document.visibilityState !== 'visible') return;
      const scroller = threadScroller;
      const top = scroller?.scrollTop ?? 0;
      const before = spacerHeightPx({ cachedHeight, controlsTop: cachedControlsTop, keyboardOpen, disabled }) + titleBarHeight;
      refreshHeight();
      updateTitleBarHeightVar();
      refreshPromptHeight();
      const after = spacerHeightPx({ cachedHeight, controlsTop: cachedControlsTop, keyboardOpen, disabled }) + titleBarHeight;
      if (scroller && top > 0 && after !== before) {
        holdAcrossRelayout(scroller as HTMLElement, Math.max(0, top + after - before));
      }
    }
    window.addEventListener('pageshow', onResume);
    document.addEventListener('visibilitychange', onResume);
    let coldStartPollFrames = 30;
    let coldStartPollId: number | null = null;
    function coldStartPoll() {
      refreshHeight();
      if (--coldStartPollFrames > 0) coldStartPollId = requestAnimationFrame(coldStartPoll);
      else coldStartPollId = null;
    }
    coldStartPollId = requestAnimationFrame(coldStartPoll);

    function onRevealBars(e: Event) {
      revealBars();
      prevScrollTop = currentContainer ? clampedScrollTop(currentContainer) : 0;
      applyTransform();
      if (promptEl && (e as CustomEvent<RevealBarsDetail | null>).detail?.instant) landWithoutGlide(promptEl);
    }
    document.addEventListener('reveal-mobile-bars', onRevealBars);

    function recomputeDisabled() {
      const nextDynamicBars = currentMobileDynamicBars();
      if (nextDynamicBars !== dynamicBars) {
        dynamicBars = nextDynamicBars;
        document.documentElement.toggleAttribute('data-mobile-dynamic-bars', dynamicBars);
        updatePromptHeightVar();
      }
      const next = shouldKeepHeaderVisible({ dynamicBars, findOpen: findSurface.value !== null });
      if (next === disabled) return;
      disabled = next;
      appTouching = false;
      if (disabled && keyboardOpen) {
        keyboardOpen = false;
        updateHeaderVar();
        if (currentContainer) {
          holdAcrossRelayout(currentContainer as HTMLElement, currentContainer.scrollTop + cachedHeight - cachedControlsTop);
        }
      }
      if (!disabled) syncToScroll(currentContainer);
      else applyTransform();
    }
    recomputeDisabled();

    const unsubRebased = onRebasedScroll((el, kind) => {
      if (el !== currentContainer) return;
      const top = clampedScrollTop(el);
      const moved = Math.abs(top - prevScrollTop) > 1;
      prevScrollTop = top;
      rebasedTop = top;
      if (revealsBars({ kind, moved })) {
        revealBars();
        measureContainer(el);
        applyTransform();
      }
      if (rebaseSettleRaf !== null) cancelAnimationFrame(rebaseSettleRaf);
      rebaseSettleRaf = requestAnimationFrame(() => {
        rebaseSettleRaf = null;
        if (el !== currentContainer) return;
        prevScrollTop = clampedScrollTop(el);
        rebasedTop = prevScrollTop;
      });
    });

    const unsub = mobileView.subscribe(() => {
      attachListener();
      refreshHeight();
    });

    const unsubPrefs = preferences.subscribe(recomputeDisabled);
    const unsubFind = findSurface.subscribe(recomputeDisabled);
    const unsubCompose = composeViewActive.subscribe(() => {
      revealBars();
      applyTransform();
    });

    return () => {
      disposed = true;
      stopContainerScroll?.();
      stopContainerScroll = null;
      document.removeEventListener('focusin', onFocusIn);
      document.removeEventListener('focusout', onFocusOut);
      document.removeEventListener('reveal-mobile-bars', onRevealBars);
      window.removeEventListener('message', onAppMessage);
      window.removeEventListener('resize', refreshHeight);
      window.visualViewport?.removeEventListener('resize', refreshHeight);
      window.removeEventListener('pageshow', onResume);
      document.removeEventListener('visibilitychange', onResume);
      if (coldStartPollId !== null) cancelAnimationFrame(coldStartPollId);
      if (mutationRafId !== null) cancelAnimationFrame(mutationRafId);
      if (rebaseSettleRaf !== null) cancelAnimationFrame(rebaseSettleRaf);
      observer.disconnect();
      headerResizeObserver.disconnect();
      if (titleBarResizeObserver) titleBarResizeObserver.disconnect();
      unsubRebased();
      unsub();
      unsubPrefs();
      unsubFind();
      unsubCompose();
      promptResizeObserver?.disconnect();
      clearPromptOffset();
      downChevronEl?.style.removeProperty('--mobile-prompt-offset');
      document.documentElement.removeAttribute('data-mobile-dynamic-bars');
      document.documentElement.style.removeProperty('--mobile-prompt-height');
      if (headerRef.current) {
        headerRef.current.style.translate = '';
      }
      document.documentElement.style.removeProperty('--mobile-header-height');
      document.documentElement.style.removeProperty('--mobile-thread-title-height');
      titleBarEl?.style.removeProperty('--mobile-header-offset');
      chevronEl?.style.removeProperty('--mobile-header-offset');
      appPanelEl?.removeAttribute(APP_PANEL_BARS_AWAY_ATTR);
    };
  }, [mobileLayout]);
}
