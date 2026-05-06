import { useEffect } from 'preact/hooks';
import { mobileView, panelOverlay } from '../store/store';
import { opensSoftwareKeyboard } from '../utils/dom';
import { getResizeMode, scrollToBottom, scrolledUp } from '../components/chat/scrollState';
import { isMobile } from '../utils/viewport';

// Selectors scoped to .mobile-swipe-pane to avoid finding the desktop elements
// (both desktop SplitLayout and mobile MobileSwipeContainer render ThreadPane/ContentPane)
const SCROLL_SELECTORS: Record<string, string> = {
  threads: '.mobile-swipe-pane .thread-drawer-list',
  thread: '.mobile-swipe-pane .thread-content.visible',
  content: '.mobile-swipe-pane .content-pane-body',
};

/**
 * Hides the fixed mobile header on scroll-down and reveals it on scroll-up,
 * tracking pixel-for-pixel in both directions (no CSS transitions).
 *
 * Clamps scrollTop to [0, maxScroll] so iOS Safari elastic bounce at the
 * bottom/top doesn't move the header.
 */
export function useHideOnScroll(headerRef: { current: HTMLElement | null }) {
  useEffect(() => {
    if (!isMobile()) return;

    let prevScrollTop = 0;
    let headerOffset = 0; // 0 = fully visible, -cachedHeight = fully hidden
    let cachedHeight = 0;
    let titleBarHeight = 0; // px, thread title bar height (0 when not on thread view)
    let titleBarEl: HTMLElement | null = null;
    let titleBarResizeObserver: ResizeObserver | null = null;
    let currentContainer: Element | null = null;
    let currentContainerPane: Element | null = null;
    let currentViewKey: string | null = null;
    let mutationRafId: number | null = null;
    let keyboardOpen = false; // true while a prompt input is focused
    let disabled = false; // true when app UI iframe is active — header stays visible
    // Per-pane scroll state so each pane has independent header position
    const paneState: Record<string, { headerOffset: number; prevScrollTop: number }> = {};
    // Actual px-per-rem — mobile uses 112.5% (18px) base font size by default
    let cachedRemSize = parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
    // Change-detection guard (avoid needless style invalidation on every scroll)
    let lastOffsetRem = 0;

    /** Clamp offset to [-(cachedHeight + titleBarHeight), 0].
     *  The extended range lets the sticky title bar scroll out at the same
     *  speed as the header. The `|| 0` prevents -0 from Math.max. */
    function clampOffset(offset: number) {
      return Math.min(0, Math.max(-(cachedHeight + titleBarHeight), offset)) || 0;
    }

    /** Set --mobile-header-height based on keyboard state.
     *  Keyboard open → 0rem (collapses spacer so content fills header space).
     *  Keyboard closed → actual header height. */
    function updateHeaderVar() {
      const heightRem = keyboardOpen ? 0 : cachedHeight / cachedRemSize;
      document.documentElement.style.setProperty('--mobile-header-height', `${heightRem}rem`);
    }

    /** Measure the sticky thread title bar and publish its height as a CSS var.
     *  The scroll-to-top chevron is positioned absolutely outside the title bar
     *  and uses this var to anchor itself just below the bar's bottom edge —
     *  including when the title wraps to multiple lines. */
    function updateTitleBarHeightVar() {
      const newHeight = titleBarEl ? titleBarEl.getBoundingClientRect().height : 0;
      if (Math.abs(newHeight - titleBarHeight) <= 0.1) return;
      titleBarHeight = newHeight;
      if (newHeight > 0) {
        document.documentElement.style.setProperty('--mobile-thread-title-height', `${newHeight / cachedRemSize}rem`);
      } else {
        document.documentElement.style.removeProperty('--mobile-thread-title-height');
      }
    }

    /** Attach a ResizeObserver to the title bar so the CSS var updates when
     *  the bar grows (e.g. title wraps to a second line on a narrow viewport). */
    function bindTitleBar(el: HTMLElement | null) {
      if (el === titleBarEl) return;
      if (titleBarResizeObserver) {
        titleBarResizeObserver.disconnect();
        titleBarResizeObserver = null;
      }
      titleBarEl = el;
      updateTitleBarHeightVar();
      if (el) {
        titleBarResizeObserver = new ResizeObserver(() => updateTitleBarHeightVar());
        titleBarResizeObserver.observe(el);
      }
    }

    function applyTransform() {
      if (headerRef.current) {
        // Disabled (app UI active) = always fully visible, regardless of scroll/keyboard
        // Keyboard open = always fully hidden, regardless of scroll state
        const offset = disabled ? 0 : keyboardOpen ? -cachedHeight : headerOffset;
        // Header transform is clamped to its own height — the extended range
        // (headerHeight + titleBarHeight) only affects the CSS var for the
        // sticky title bar, not the header element itself.
        const headerTranslate = Math.max(-cachedHeight, offset);
        headerRef.current.style.transform = headerTranslate !== 0
          ? `translateY(${headerTranslate / cachedRemSize}rem)` : '';
        // Expose full offset (including extended range) for the sticky thread
        // title bar. Guarded to avoid needless style invalidation.
        const offsetRem = offset / cachedRemSize;
        if (offsetRem !== lastOffsetRem) {
          lastOffsetRem = offsetRem;
          document.documentElement.style.setProperty('--mobile-header-offset', `${offsetRem}rem`);
        }
      }
    }

    /** Sync headerOffset to match a container's scroll position (keyboard dismiss). */
    function syncToScroll(container: Element | null) {
      if (container) {
        const scrollPos = Math.max(0, container.scrollTop);
        headerOffset = clampOffset(-scrollPos);
        prevScrollTop = scrollPos;
      } else {
        headerOffset = 0;
        prevScrollTop = 0;
      }
      applyTransform();
    }

    function refreshHeight() {
      cachedRemSize = parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
      const el = headerRef.current;
      if (!el) {
        if (cachedHeight !== 0) {
          cachedHeight = 0;
          updateHeaderVar();
        }
        return;
      }
      // getBoundingClientRect gives subpixel accuracy — offsetHeight truncates
      // to integer, leaving a fractional-pixel gap between the fixed header and
      // sticky/spacer elements below it. Do NOT revert to offsetHeight.
      const h = el.getBoundingClientRect().height;
      if (Math.abs(h - cachedHeight) > 0.1) {
        cachedHeight = h;
        updateHeaderVar();
      }
    }

    function onScroll() {
      const header = headerRef.current;
      if (!header || cachedHeight === 0 || !currentContainer || disabled) return;

      recoverKeyboardState();

      const rawScrollTop = currentContainer.scrollTop;
      const maxScroll = Math.max(0, currentContainer.scrollHeight - currentContainer.clientHeight);
      const scrollTop = Math.min(Math.max(0, rawScrollTop), maxScroll);

      // Programmatic scroll (scrollToBottom) — reset header to visible so
      // opening a thread shows the mobile header, not hides it.
      if (getResizeMode() === 'scroll') {
        prevScrollTop = scrollTop;
        headerOffset = 0;
        applyTransform();
        return;
      }

      // iOS Safari doesn't always blur inputs when swiping between scroll-snap panes.
      const active = document.activeElement;
      if (active && opensSoftwareKeyboard(active)) {
        const activePane = active.closest('.mobile-swipe-pane');
        if (activePane === currentContainerPane) return;
      }

      const delta = scrollTop - prevScrollTop;

      headerOffset = clampOffset(headerOffset - delta);
      applyTransform();
      prevScrollTop = scrollTop;
    }

    /** Recover from stale keyboard state. iOS Safari doesn't always fire
     *  focusout when swiping between scroll-snap panes, leaving keyboardOpen
     *  stuck true — which permanently hides the header and collapses the spacer. */
    function recoverKeyboardState() {
      if (!keyboardOpen) return;
      const active = document.activeElement;
      if (active && opensSoftwareKeyboard(active)) return;
      keyboardOpen = false;
      updateHeaderVar();
    }

    function attachListener() {
      const view = mobileView.value;
      const selector = SCROLL_SELECTORS[view];
      if (!selector) return;

      const container = document.querySelector(selector);

      // Check before early return — runs on every pane switch AND
      // every MutationObserver callback, even when container is unchanged.
      recoverKeyboardState();

      if (container === currentContainer) return;

      if (currentViewKey) {
        paneState[currentViewKey] = { headerOffset, prevScrollTop };
      }

      if (currentContainer) {
        currentContainer.removeEventListener('scroll', onScroll);
      }

      currentContainer = container;
      currentContainerPane = container?.closest('.mobile-swipe-pane') ?? null;
      currentViewKey = view;
      // Measure title bar height — only present on thread view. ResizeObserver
      // keeps the CSS var fresh as the title wraps/unwraps.
      bindTitleBar(container?.querySelector('.mobile-thread-title-row') as HTMLElement | null);
      refreshHeight();
      if (container) {
        // Restore this pane's saved scroll state, or derive from scroll position
        const saved = paneState[view];
        if (saved) {
          headerOffset = saved.headerOffset;
          prevScrollTop = saved.prevScrollTop;
        } else {
          const scrollPos = Math.max(0, container.scrollTop);
          headerOffset = clampOffset(-scrollPos);
          prevScrollTop = scrollPos;
        }
        // Verify saved state matches actual scroll — if the container is
        // near the top, the header+title must be visible regardless of saved state.
        // Prevents stale negative offset from a different view leaking in.
        const actualScroll = Math.max(0, container.scrollTop);
        if (actualScroll < cachedHeight + titleBarHeight) {
          headerOffset = clampOffset(-actualScroll);
          prevScrollTop = actualScroll;
        }
        container.addEventListener('scroll', onScroll, { passive: true });
      } else {
        // No scroll container — reset scroll-based offset. applyTransform
        // still hides the header if keyboard is open (keyboardOpen flag).
        headerOffset = 0;
        prevScrollTop = 0;
      }
      applyTransform();
    }

    refreshHeight();
    attachListener();

    // Hide header and collapse spacer when any prompt input gains focus.
    // applyTransform checks keyboardOpen independently of headerOffset,
    // so this can't race with attachListener or scroll updates.
    function onFocusIn(e: FocusEvent) {
      if (!opensSoftwareKeyboard(e.target)) return;
      const target = e.target as HTMLElement;
      if (headerRef.current?.contains(target)) return;
      // Title bar is inside the scroll pane (not the header) but should
      // behave like a header input — don't hide when editing the title.
      if (target.closest('.mobile-thread-title-row')) return;
      if (keyboardOpen) return; // Already hidden — skip duplicate scroll compensation
      const wasAtBottom = !scrolledUp.value;
      keyboardOpen = true;
      updateHeaderVar();
      // Compensate scroll: spacer collapsed, content shifted up
      if (currentContainer) {
        currentContainer.scrollTop = Math.max(0, currentContainer.scrollTop - cachedHeight);
      }
      applyTransform();
      // Scroll compensation corrupts scrolledUp via the scroll event it fires.
      // Restore bottom-pinned state through the keyboard open animation.
      if (wasAtBottom) scrollToBottom();
    }
    document.addEventListener('focusin', onFocusIn);

    function onFocusOut(e: FocusEvent) {
      if (!opensSoftwareKeyboard(e.target)) return;
      if (headerRef.current?.contains(e.target as HTMLElement)) return;
      // Only undo scroll compensation if we actually collapsed the spacer.
      // Prevents spurious scroll jumps from inputs excluded in onFocusIn
      // (e.g., .mobile-thread-title-row).
      if (!keyboardOpen) return;
      // Focus moving to another text input (e.g. prompt → CC menu filter):
      // skip header restore — the subsequent focusin keeps keyboardOpen true
      // without a visible flash.
      const next = e.relatedTarget as HTMLElement | null;
      if (next && opensSoftwareKeyboard(next) && !headerRef.current?.contains(next)
          && !next.closest('.mobile-thread-title-row')) return;
      const wasAtBottom = !scrolledUp.value;
      keyboardOpen = false;
      updateHeaderVar();
      // Compensate scroll: spacer restored, content shifted down
      if (currentContainer) {
        currentContainer.scrollTop += cachedHeight;
      }
      syncToScroll(currentContainer);
      // Same as onFocusIn: scroll compensation corrupts scrolledUp.
      // Restore bottom-pinned state through the keyboard close animation.
      if (wasAtBottom) scrollToBottom();
    }
    document.addEventListener('focusout', onFocusOut);

    // Re-attach when DOM updates (e.g., thread-content appears after loading).
    // Debounced via rAF to avoid churn during heavy DOM mutations.
    //
    // childList only — NOT attributes. Each rAF runs getBoundingClientRect +
    // scrollTop reads (forced layout). In large workspaces with active
    // streaming, watching class changes fired this every frame from Preact
    // class flips, blocking the compositor and janking pane swipes.
    // The scroll-target elements (.thread-content.visible, .thread-drawer-list,
    // .content-pane-body) mount/unmount as units — childList catches every
    // real container change.
    const observer = new MutationObserver(() => {
      if (mutationRafId !== null) return;
      mutationRafId = requestAnimationFrame(() => {
        mutationRafId = null;
        attachListener();
        refreshHeight();
        // Correct header if scroll position warrants more visibility
        // than the current offset provides. attachListener() only runs
        // this check when the container element changes, but content can
        // shrink (e.g. steps collapsed, CC session finished) without the
        // container changing — leaving the header stuck hidden.
        if (currentContainer && cachedHeight > 0 && !keyboardOpen && !disabled) {
          const actualScroll = Math.max(0, currentContainer.scrollTop);
          if (actualScroll < cachedHeight + titleBarHeight) {
            const corrected = clampOffset(-actualScroll);
            if (headerOffset < corrected) {
              headerOffset = corrected;
              prevScrollTop = actualScroll;
              applyTransform();
            }
          }
        }
      });
    });
    const swipeWrapper = document.querySelector('.mobile-swipe-wrapper');
    if (swipeWrapper) {
      observer.observe(swipeWrapper, { childList: true, subtree: true });
    }

    // Track header height changes (e.g. thread title row appearing/disappearing)
    // so --mobile-header-height stays accurate for scroll spacers.
    const headerResizeObserver = new ResizeObserver(() => {
      refreshHeight();
    });
    if (headerRef.current) headerResizeObserver.observe(headerRef.current);

    // Reveal header when a change is applied/discarded/reverted — the user
    // is typically scrolled far down and the header is hidden, but the state
    // transition warrants showing the app header again.
    function onRevealHeader() {
      headerOffset = 0;
      prevScrollTop = currentContainer ? Math.max(0, currentContainer.scrollTop) : 0;
      applyTransform();
    }
    document.addEventListener('reveal-mobile-header', onRevealHeader);

    const unsub = mobileView.subscribe(() => {
      attachListener();
      refreshHeight();
    });

    // Disable hide-on-scroll when an app UI iframe is active.
    // Iframe scroll events can propagate unpredictably to the parent,
    // causing the header to flicker or get stuck. Keeping the header
    // fixed visible eliminates the race condition entirely.
    const unsubOverlay = panelOverlay.subscribe((overlay) => {
      const newDisabled = overlay?.type === 'app-ui';
      if (newDisabled !== disabled) {
        disabled = newDisabled;
        // Reset stale keyboard state when entering disabled mode.
        // iOS Safari may miss focusout when opening app UI, leaving
        // keyboardOpen true → spacer collapses while header is forced visible.
        if (disabled && keyboardOpen) {
          keyboardOpen = false;
          updateHeaderVar();
        }
        applyTransform();
      }
    });

    return () => {
      if (currentContainer) {
        currentContainer.removeEventListener('scroll', onScroll);
      }
      document.removeEventListener('focusin', onFocusIn);
      document.removeEventListener('focusout', onFocusOut);
      document.removeEventListener('reveal-mobile-header', onRevealHeader);
      if (mutationRafId !== null) cancelAnimationFrame(mutationRafId);
      observer.disconnect();
      headerResizeObserver.disconnect();
      if (titleBarResizeObserver) titleBarResizeObserver.disconnect();
      unsub();
      unsubOverlay();
      if (headerRef.current) {
        headerRef.current.style.transform = '';
      }
      document.documentElement.style.removeProperty('--mobile-header-height');
      document.documentElement.style.removeProperty('--mobile-header-offset');
      document.documentElement.style.removeProperty('--mobile-thread-title-height');
    };
  }, []);
}
