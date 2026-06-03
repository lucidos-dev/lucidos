import { useEffect } from 'preact/hooks';
import { mobileView, panelOverlay, preferences, type MobileView, type PanelOverlay } from '../store/store';
import { opensSoftwareKeyboard, getRemPx } from '../utils/dom';
import { getResizeMode, pinToBottomNow, scrolledUp } from '../components/chat/scrollState';
import { isMobile } from '../utils/viewport';
import { currentMobileHeaderSticky } from '../store/actions/preferences';

/** Pure rule for when hide-on-scroll should be inert and the header pinned visible.
 *  Sticky preference wins everywhere; otherwise only the content pane with an
 *  open app-UI iframe pins (its scroll events leak to the parent — other panes
 *  don't have that problem and should hide normally). */
export function shouldKeepHeaderVisible(opts: {
  view: MobileView;
  overlayType: NonNullable<PanelOverlay>['type'] | null | undefined;
  stickyPref: boolean;
}): boolean {
  if (opts.stickyPref) return true;
  return opts.view === 'content' && opts.overlayType === 'app-ui';
}

/** Pixel height for the `--mobile-header-height` spacer that sits below the
 *  fixed header. Collapses to the safe-area inset only while the header is
 *  actually sliding off to make room for the keyboard. When the header is
 *  pinned visible (`disabled` — sticky pref or app-ui), it never slides off,
 *  so the spacer MUST stay at full header height — otherwise content slides up
 *  behind the still-visible header (editing a device name on an iOS PWA with
 *  "Keep header visible" on rendered the input under the header). */
export function spacerHeightPx(opts: {
  cachedHeight: number;
  safeAreaTop: number;
  keyboardOpen: boolean;
  disabled: boolean;
}): number {
  return opts.keyboardOpen && !opts.disabled ? opts.safeAreaTop : opts.cachedHeight;
}

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
    // Header's padding-top (env(safe-area-inset-top)) in px. See updateHeaderVar.
    let cachedSafeAreaTop = 0;
    let titleBarHeight = 0; // px, thread title bar height (0 when not on thread view)
    let titleBarEl: HTMLElement | null = null;
    let titleBarResizeObserver: ResizeObserver | null = null;
    let currentContainer: Element | null = null;
    let currentContainerPane: Element | null = null;
    let currentViewKey: string | null = null;
    let mutationRafId: number | null = null;
    let keyboardOpen = false; // true while a prompt input is focused
    let disabled = false;
    // Per-pane scroll state so each pane has independent header position
    const paneState: Record<string, { headerOffset: number; prevScrollTop: number }> = {};
    let cachedRemSize = getRemPx();
    // Change-detection guard (avoid needless style invalidation on every scroll)
    let lastOffsetRem = 0;

    /** Clamp offset to [-(cachedHeight + titleBarHeight), 0].
     *  The extended range lets the sticky title bar scroll out at the same
     *  speed as the header. The `|| 0` prevents -0 from Math.max. */
    function clampOffset(offset: number) {
      return Math.min(0, Math.max(-(cachedHeight + titleBarHeight), offset)) || 0;
    }

    /** Set --mobile-header-height based on keyboard + pinned state.
     *  Keyboard open (header sliding off) → safe-area-inset-top: keeps a spacer
     *    at the notch / dynamic-island clearance so focused inputs near the top
     *    of the page don't end up behind iOS chrome. Resolves to 0 on platforms
     *    without a safe-area inset (Android, pre-notch iPhones, desktop).
     *  Header pinned visible (sticky pref / app-ui) or keyboard closed → actual
     *    header height, so content stays clear of the still-visible header. */
    function updateHeaderVar() {
      const heightPx = spacerHeightPx({
        cachedHeight,
        safeAreaTop: cachedSafeAreaTop,
        keyboardOpen,
        disabled,
      });
      const heightRem = heightPx / cachedRemSize;
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
      cachedRemSize = getRemPx();
      const el = headerRef.current;
      if (!el) {
        if (cachedHeight !== 0 || cachedSafeAreaTop !== 0) {
          cachedHeight = 0;
          cachedSafeAreaTop = 0;
          updateHeaderVar();
        }
        return;
      }
      // getBoundingClientRect gives subpixel accuracy — offsetHeight truncates
      // to integer, leaving a fractional-pixel gap between the fixed header and
      // sticky/spacer elements below it. Do NOT revert to offsetHeight.
      const h = el.getBoundingClientRect().height;
      if (Math.abs(h - cachedHeight) <= 0.1) return;
      cachedHeight = h;
      // padding-top is env(safe-area-inset-top, 0px) (mobile.css). Any change
      // to it also changes the bounding height, so we only re-read when the
      // height delta above already proved something moved — skipping a forced
      // style flush on every no-op refreshHeight tick.
      cachedSafeAreaTop = parseFloat(getComputedStyle(el).paddingTop) || 0;
      updateHeaderVar();
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

    /** iOS Safari sometimes misses focusout — when swiping scroll-snap panes,
     *  or when the focused input is removed mid-edit. syncToScroll re-applies
     *  the transform; updateHeaderVar alone leaves the header stuck at
     *  translateY(-cachedHeight) above the viewport. */
    function recoverKeyboardState() {
      if (!keyboardOpen) return;
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
      // Header pinned visible (sticky pref / app-ui): it never slides off for
      // the keyboard, so the spacer must stay full-height. Collapsing it here
      // would slide content up behind the still-visible header (editing a
      // device name on an iOS PWA rendered the input under the header).
      if (disabled) return;
      if (keyboardOpen) return; // Already hidden — skip duplicate scroll compensation
      const wasAtBottom = !scrolledUp.value;
      keyboardOpen = true;
      updateHeaderVar();
      // Compensate scroll: spacer shrinks from cachedHeight to cachedSafeAreaTop,
      // so move content up by the same delta to stay visually anchored.
      if (currentContainer) {
        const delta = cachedHeight - cachedSafeAreaTop;
        currentContainer.scrollTop = Math.max(0, currentContainer.scrollTop - delta);
      }
      applyTransform();
      // Scroll compensation corrupts scrolledUp via the scroll event it fires.
      // Restore bottom-pinned state through the keyboard open animation.
      if (wasAtBottom) pinToBottomNow();
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
      // Mirror onFocusIn: spacer grows back, so add the same delta to scrollTop.
      if (currentContainer) {
        currentContainer.scrollTop += cachedHeight - cachedSafeAreaTop;
      }
      syncToScroll(currentContainer);
      // Same as onFocusIn: scroll compensation corrupts scrolledUp.
      // Restore bottom-pinned state through the keyboard close animation.
      if (wasAtBottom) pinToBottomNow();
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
        // shrink (e.g. steps collapsed, Claude Code session finished) without the
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

    // Track header height changes (e.g. thread title row appearing/disappearing,
    // safe-area-inset-top resolving from 0 to its on-device value during iOS
    // PWA layout settle / orientation changes) so --mobile-header-height stays
    // accurate for scroll spacers. observe() defaults to content-box, which
    // does NOT change when only the header's padding-top env() inset changes —
    // the spacer would then keep its pre-inset size and the first row of the
    // threads list peeks behind the header. Watch the border-box explicitly.
    const headerResizeObserver = new ResizeObserver(() => {
      refreshHeight();
    });
    if (headerRef.current) headerResizeObserver.observe(headerRef.current, { box: 'border-box' });

    // Belt-and-braces for iOS PWA: ResizeObserver alone is not reliable on
    // WebKit when env(safe-area-inset-top) resolves from 0 to its real value
    // a tick after first paint (cold start, return-from-background, orientation
    // change). The visible symptom is the entire section title row sitting
    // behind the header — the spacer is sized for env=0 and never grows.
    // Catch the same transitions through three independent triggers:
    //  - window.resize: orientation change, viewport size change.
    //  - visualViewport.resize: viewport-relative changes including the
    //    on-screen keyboard appearing/disappearing AND env() resolution on
    //    iOS PWA (the visualViewport's height adapts to the resolved insets).
    //  - rAF poll for the first ~500ms after mount: covers the cold-start
    //    case where env() lands AFTER useEffect runs but BEFORE any of the
    //    above events fire. A handful of frames is enough — anything beyond
    //    that lives on the ResizeObserver / event listeners.
    window.addEventListener('resize', refreshHeight);
    window.visualViewport?.addEventListener('resize', refreshHeight);
    let coldStartPollFrames = 30; // ~500ms at 60fps
    let coldStartPollId: number | null = null;
    function coldStartPoll() {
      refreshHeight();
      if (--coldStartPollFrames > 0) coldStartPollId = requestAnimationFrame(coldStartPoll);
      else coldStartPollId = null;
    }
    coldStartPollId = requestAnimationFrame(coldStartPoll);

    // Reveal header when a change is applied/discarded/reverted — the user
    // is typically scrolled far down and the header is hidden, but the state
    // transition warrants showing the app header again.
    function onRevealHeader() {
      headerOffset = 0;
      prevScrollTop = currentContainer ? Math.max(0, currentContainer.scrollTop) : 0;
      applyTransform();
    }
    document.addEventListener('reveal-mobile-header', onRevealHeader);

    function recomputeDisabled() {
      const next = shouldKeepHeaderVisible({
        view: mobileView.value,
        overlayType: panelOverlay.value?.type,
        stickyPref: currentMobileHeaderSticky(),
      });
      if (next === disabled) return;
      disabled = next;
      // Reset stale keyboard state when pinning the header. iOS Safari may
      // miss focusout when opening app UI, leaving keyboardOpen true →
      // spacer would collapse while header is forced visible.
      if (disabled && keyboardOpen) {
        keyboardOpen = false;
        updateHeaderVar();
      }
      applyTransform();
    }
    recomputeDisabled();

    const unsub = mobileView.subscribe(() => {
      attachListener();
      refreshHeight();
      recomputeDisabled();
    });

    const unsubOverlay = panelOverlay.subscribe(recomputeDisabled);
    const unsubPrefs = preferences.subscribe(recomputeDisabled);

    return () => {
      if (currentContainer) {
        currentContainer.removeEventListener('scroll', onScroll);
      }
      document.removeEventListener('focusin', onFocusIn);
      document.removeEventListener('focusout', onFocusOut);
      document.removeEventListener('reveal-mobile-header', onRevealHeader);
      window.removeEventListener('resize', refreshHeight);
      window.visualViewport?.removeEventListener('resize', refreshHeight);
      if (coldStartPollId !== null) cancelAnimationFrame(coldStartPollId);
      if (mutationRafId !== null) cancelAnimationFrame(mutationRafId);
      observer.disconnect();
      headerResizeObserver.disconnect();
      if (titleBarResizeObserver) titleBarResizeObserver.disconnect();
      unsub();
      unsubOverlay();
      unsubPrefs();
      if (headerRef.current) {
        headerRef.current.style.transform = '';
      }
      document.documentElement.style.removeProperty('--mobile-header-height');
      document.documentElement.style.removeProperty('--mobile-header-offset');
      document.documentElement.style.removeProperty('--mobile-thread-title-height');
    };
  }, []);
}
