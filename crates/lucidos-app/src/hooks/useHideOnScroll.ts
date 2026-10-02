import { useEffect } from 'preact/hooks';
import { composeViewActive, mobileView, panelOverlay, preferences, type MobileView, type PanelOverlay } from '../store/store';
import { opensSoftwareKeyboard, getRemPx } from '../utils/dom';
import { afterPressSettles } from '../utils/pointerPress';
import {
  holdAcrossRelayout,
  isAnchorScroll,
  isCarryScroll,
  isNavigationScroll,
  isHeaderPinnedForScroll,
  onRebasedScroll,
  type NavScrollKind,
} from '../components/chat/scrollState';
import { viewportIsMobile } from '../utils/viewport';
import { isRepaintNudging } from '../utils/webkitRepaint';
import { isUserScrolling } from '../utils/scrollActivity';
import { currentMobileDynamicBars } from '../store/actions/preferences';

/** Pure rule for when hide-on-scroll should be inert and the header pinned visible.
 *  Pinned bars (dynamic bars off) win everywhere. Otherwise only the content
 *  pane with an open app-UI iframe pins, because its scroll events leak to the
 *  parent. Other panes don't have that problem and hide normally. */
export function shouldKeepHeaderVisible(opts: {
  view: MobileView;
  overlayType: NonNullable<PanelOverlay>['type'] | null | undefined;
  dynamicBars: boolean;
}): boolean {
  if (!opts.dynamicBars) return true;
  return opts.view === 'content' && opts.overlayType === 'app-ui';
}

/** Pixel height for the `--mobile-header-height` spacer that sits below the
 *  fixed header. Collapses to the safe-area inset only while the header is
 *  actually sliding off to make room for the keyboard. When the header is
 *  pinned visible (`disabled`: pinned bars or app-ui), it never slides off,
 *  so the spacer MUST stay at full header height — otherwise content slides up
 *  behind the still-visible header (editing a device name on an iOS PWA with
 *  the bars pinned rendered the input under the header). */
export function spacerHeightPx(opts: {
  cachedHeight: number;
  safeAreaTop: number;
  keyboardOpen: boolean;
  disabled: boolean;
}): number {
  return opts.keyboardOpen && !opts.disabled ? opts.safeAreaTop : opts.cachedHeight;
}

/** Whether a scroll event's delta counts toward the bars' intent. Only the
 *  reader's own scroll does. Every write of ours re-bases instead, as does the
 *  window a deep link pins (ADR 0337). */
export function countsAsReaderScroll(opts: {
  navigation: boolean;
  headerPinned: boolean;
}): boolean {
  return !opts.headerPinned && !opts.navigation;
}

/** Whether one of our own writes brings the bars back. A placement or a held
 *  write takes the reader somewhere. An anchor write keeps them on their line,
 *  and the follow's carry rides a reply, so neither moves the bars (ADR 0337).
 *
 *  A write that moved nothing took nobody anywhere. Focus landing on a control
 *  already on screen marks one (`markRevealScroll`), on every tap. */
export function revealsBars(opts: { kind: NavScrollKind; moved: boolean }): boolean {
  return opts.moved && (opts.kind === 'placement' || opts.kind === 'held');
}

/** The `reveal-mobile-bars` event's detail. `instant` lands the prompt with no
 *  glide, for a caller about to focus it: iOS Safari can refuse the keyboard
 *  to a prompt still under a `translate`. */
export interface RevealBarsDetail {
  instant: boolean;
}

/** End any glide on `el` at once. The style flush with the transition off is
 *  what lands it, and restoring the transition afterwards starts none. */
export function landWithoutGlide(el: HTMLElement) {
  el.style.transition = 'none';
  void getComputedStyle(el).translate;
  el.style.transition = '';
}

/** How far the reader must travel in one direction before the bars follow it,
 *  in rem. Shorter travel is jitter, and must not flip them. */
export const BARS_TRAVEL_REM = 0.75;

/** Where the reader's scrolling says the bars belong. `travel` is the signed
 *  distance moved since the direction last changed, down positive. */
export interface BarsIntent {
  away: boolean;
  travel: number;
}

export const BARS_SHOWN: BarsIntent = { away: false, travel: 0 };

/** Fold one scroll delta into the intent. It flips only once the travel in
 *  the new direction reaches `thresholdPx`, so the bars never flicker. */
export function nextBarsIntent(intent: BarsIntent, delta: number, thresholdPx: number): BarsIntent {
  if (delta === 0) return intent;
  const sameWay = Math.sign(delta) === Math.sign(intent.travel);
  const travel = sameWay ? intent.travel + delta : delta;
  if (travel >= thresholdPx) return { away: true, travel };
  if (travel <= -thresholdPx) return { away: false, travel };
  return { away: intent.away, travel };
}

/** The chrome's offset, in px: 0 shown, `-chromeHeight` away. The chrome is
 *  the header, plus the title bar on the thread pane. Within the chrome's own
 *  height of the top it stays shown, or the top of the pane would show an
 *  empty band where it was. */
export function headerOffsetPx(opts: { away: boolean; scrollTop: number; chromeHeight: number }): number {
  return opts.away && opts.scrollTop > opts.chromeHeight ? -opts.chromeHeight : 0;
}

/** The chrome's offset with the pin and the keyboard folded in.
 *
 *  The keyboard sends the chrome away by the same distance a scroll does, title
 *  bar included. Moving it by the header alone parks the title bar just above
 *  the screen. The spacer growing back on the keyboard close then pops it to
 *  the top in one frame, ahead of the header gliding in. */
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

/** The thread title bar's offset, in px. On its own pane it is the chrome's.
 *  On any other pane it follows the header away. A swipe back then never
 *  shows the transcript in the band above a bar under an absent header.
 *
 *  The transcript reaches that band only once it has scrolled past the bar's
 *  own height. Up to there the band is the empty top of the pane, and the bar
 *  stays shown: hidden, it would leave its own empty band where it rests. */
export function titleBarOffsetPx(opts: {
  onThreadPane: boolean;
  chromeOffset: number;
  threadScrollTop: number;
  headerHeight: number;
  titleBarHeight: number;
}): number {
  if (opts.onThreadPane) return opts.chromeOffset;
  const away = opts.chromeOffset < 0 && opts.threadScrollTop > opts.titleBarHeight;
  return away ? -(opts.headerHeight + opts.titleBarHeight) : 0;
}

/** The prompt's offset below its resting place, in px: 0 shown,
 *  `promptHeight` away. On its own pane it follows the reader's scroll. On any
 *  other pane it follows the header away, so a swipe back glides both in
 *  together.
 *
 *  Within a prompt height of the transcript's bottom it stays shown, so
 *  nothing at the live edge sits under it. */
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

/** Whether the prompt may be anywhere but fully shown. It stays shown while the
 *  reader types into it and in the compose-empty view. It also stays shown while
 *  the bars are pinned (`disabled`, which dynamic bars off always implies). */
export function promptCanSlide(opts: {
  disabled: boolean;
  keyboardOpen: boolean;
  composeEmpty: boolean;
}): boolean {
  return !opts.disabled && !opts.keyboardOpen && !opts.composeEmpty;
}

// Selectors scoped to .mobile-swipe-pane to avoid finding the desktop elements
// (both desktop SplitLayout and mobile MobileSwipeContainer render ThreadPane/ContentPane)
const SCROLL_SELECTORS: Record<string, string> = {
  threads: '.mobile-swipe-pane .thread-drawer-list',
  thread: '.mobile-swipe-pane .thread-content.visible',
  content: '.mobile-swipe-pane .content-pane-body',
};

/**
 * Dynamic bars: the mobile header, the thread's title bar and its prompt glide
 * away on a scroll down and back on a scroll up.
 *
 * The scroll only decides WHERE they belong (`nextBarsIntent`). A CSS transition
 * on `translate` moves them there, and the compositor runs it. Per-event motion
 * trails an iOS scroll, whose events arrive late and stall while the main thread
 * is busy (ADR 0336).
 *
 * No scroll but the reader's own moves them. A reply scrolling the thread holds
 * them where they are (ADR 0337). Our own navigations, a pane swipe and the
 * keyboard closing bring them back.
 *
 * Clamps scrollTop to [0, maxScroll] so iOS Safari elastic bounce at the
 * bottom/top doesn't move the header.
 *
 * Lives as long as the mobile layout, not the header. A viewport that leaves
 * the phone layout (a tablet's split, a widened window) swaps to the desktop
 * layout, and the header survives the swap. Every element bound below belongs
 * to the mobile tree that the swap destroys.
 */
export function useHideOnScroll(headerRef: { current: HTMLElement | null }) {
  const mobileLayout = viewportIsMobile.value;
  useEffect(() => {
    if (!mobileLayout) return;

    /** The delta baseline: the scroll position the last event was measured at. */
    let prevScrollTop = 0;
    let intent = BARS_SHOWN;
    /** The container's last measured position, which the edge rules read. */
    let viewScrollTop = 0;
    let distanceToBottomPx = Infinity;
    let cachedHeight = 0;
    // Header's padding-top (var(--safe-area-top)) in px. See updateHeaderVar.
    let cachedSafeAreaTop = 0;
    let titleBarHeight = 0; // px, thread title bar height (0 while no thread is open)
    let titleBarEl: HTMLElement | null = null;
    // The thread's transcript, which the title bar and the prompt read while
    // another pane is current.
    let threadScroller: Element | null = null;
    // A freshly bound title bar has no position to glide from, so it lands.
    let titleBarFresh = false;
    // The scroll-to-top chevron, the other `--mobile-header-offset` consumer.
    let chevronEl: HTMLElement | null = null;
    let titleBarResizeObserver: ResizeObserver | null = null;
    let currentContainer: Element | null = null;
    let currentContainerPane: Element | null = null;
    let currentViewKey: string | null = null;
    let mutationRafId: number | null = null;
    /** The pending next-frame re-base after one of our own writes
     *  (`onRebasedScroll`). */
    let rebaseSettleRaf: number | null = null;
    /** Where our last write left this container, or -1 for none outstanding.
     *  `onScroll` reads it to recognise that write's own event however late it
     *  lands. See the reveal in `onScroll`. */
    let rebasedTop = -1;
    let keyboardOpen = false; // true while a prompt input is focused
    let disposed = false; // a deferred focusout must not write after cleanup
    let closePending = false; // a focusout is waiting for its press to click
    let disabled = false;
    // Mirrors `mobile_dynamic_bars`, and gates the prompt's overlay layout.
    let dynamicBars = false;
    let cachedRemSize = getRemPx();
    // Change-detection guards (avoid needless style invalidation on every scroll)
    let lastOffsetRem = 0;
    let lastTitleOffsetRem = 0;
    // The thread pane's prompt and the scroll-to-bottom chevron above it,
    // whichever pane is current: off its pane the prompt follows the header.
    let promptEl: HTMLElement | null = null;
    let downChevronEl: HTMLElement | null = null;
    let promptResizeObserver: ResizeObserver | null = null;
    let promptHeight = 0;
    let lastPromptOffsetRem = 0;

    /** Bring every bar back, and start the direction afresh. */
    function revealBars() {
      intent = BARS_SHOWN;
    }

    /** Re-read the position the edge rules depend on. */
    function measureContainer(container: Element) {
      viewScrollTop = clampedScrollTop(container);
      distanceToBottomPx = distanceToBottom(container, viewScrollTop);
    }

    function distanceToBottom(container: Element, scrollTop = clampedScrollTop(container)): number {
      return Math.max(0, container.scrollHeight - container.clientHeight) - scrollTop;
    }

    /** Set --mobile-header-height based on keyboard + pinned state.
     *  Keyboard open (header sliding off) → safe-area-inset-top: keeps a spacer
     *    at the notch / dynamic-island clearance so focused inputs near the top
     *    of the page don't end up behind iOS chrome. Resolves to 0 on platforms
     *    without a safe-area inset (Android, pre-notch iPhones, desktop).
     *  Header pinned visible (pinned bars / app-ui) or keyboard closed → actual
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
      titleBarFresh = el !== null;
      // The new element carries no offset yet, whatever the guard remembers.
      lastTitleOffsetRem = NaN;
      updateTitleBarHeightVar();
      if (el) {
        // A bar away off screen must clear it by its new height, too.
        titleBarResizeObserver = new ResizeObserver(() => {
          updateTitleBarHeightVar();
          applyTransform();
        });
        titleBarResizeObserver.observe(el);
      }
    }

    /** Bind the two elements that CONSUME `--mobile-header-offset`, so the
     *  offset write can target them instead of `documentElement`.
     *
     *  Custom properties inherit, so setting one on the root invalidates style
     *  for every node in the document, and the transcript is the largest tree
     *  in the app. Writing it on the consumers narrows invalidation to two
     *  tiny subtrees. The CSS needs no change either way, since `var()` resolves
     *  a custom property from the element's own computed value.
     *
     *  Deliberately NOT back to `documentElement` even though that would keep a
     *  future third consumer working for free: a third consumer is exactly the
     *  thing that should have to opt in here, rather than silently reinstating
     *  the document-wide recalc.
     *
     *  Merging the two headers into one container would delete this var
     *  outright, and is off the table: the title bar lives inside the scroll
     *  container ON PURPOSE, so it swipes with the pane rather than sitting
     *  frozen while the pane slides out from under it (`766f53acd`, and
     *  `MobileThreadTitleBar`'s own doc comment). */
    function bindOffsetConsumers(container: Element | null) {
      threadScroller = document.querySelector(SCROLL_SELECTORS.thread);
      // The thread pane's title bar, whichever pane is current: it follows the
      // header everywhere (`titleBarOffsetPx`), so it is never found stale.
      const nextTitleBar = document.querySelector<HTMLElement>(`${SCROLL_SELECTORS.thread} .mobile-thread-title-row`);
      // ScrollControls renders as a SIBLING of the scroll container, not inside
      // it, so this reaches up one level. `:scope >` keeps it to THIS pane's
      // chevron rather than the first one in document order.
      const nextChevron = (container?.parentElement?.querySelector(':scope > .scroll-to-top') ?? null) as HTMLElement | null;
      // The down chevron is the third consumer, of the PROMPT's offset: it sits
      // just above the prompt, so it has to slide with it on every pane.
      const nextDownChevron = (threadScroller?.parentElement?.querySelector(':scope > .scroll-to-bottom') ?? null) as HTMLElement | null;
      if (nextTitleBar === titleBarEl && nextChevron === chevronEl && nextDownChevron === downChevronEl) return false;
      // The outgoing up chevron keeps its last value. A pane swipe has already
      // revealed it (`attachListener`), so it glides in as it slides out.
      bindTitleBar(nextTitleBar);
      chevronEl = nextChevron;
      downChevronEl = nextDownChevron;
      // Force the next applyTransform to write. The freshly-bound elements carry
      // no value (or a stale one), and the change-detection guard below would
      // otherwise skip them because the OFFSET itself has not moved.
      lastOffsetRem = NaN;
      lastPromptOffsetRem = NaN;
      return true;
    }

    /** Publish `--mobile-prompt-height`, the transcript's bottom spacer under
     *  the overlaid prompt (styles/mobile.css). It moves only when the prompt
     *  resizes, never per scroll frame, so the root is the right place for it. */
    function updatePromptHeightVar() {
      const root = document.documentElement.style;
      if (dynamicBars && promptHeight > 0) {
        root.setProperty('--mobile-prompt-height', `${promptHeight / cachedRemSize}rem`);
      } else {
        root.removeProperty('--mobile-prompt-height');
      }
    }

    function refreshPromptHeight() {
      promptHeight = promptEl ? promptEl.getBoundingClientRect().height : 0;
      updatePromptHeightVar();
      applyTransform();
    }

    /** Bind the thread pane's prompt, the element the prompt offset moves. It
     *  mounts with the swipe panes, which can land after this hook's effect. */
    function bindPrompt() {
      const next = document.querySelector<HTMLElement>('.mobile-swipe-pane .prompt-area');
      if (next === promptEl) return;
      promptResizeObserver?.disconnect();
      promptResizeObserver = null;
      if (promptEl) promptEl.style.translate = '';
      promptEl = next;
      lastPromptOffsetRem = NaN;
      if (next) {
        promptResizeObserver = new ResizeObserver(refreshPromptHeight);
        promptResizeObserver.observe(next, { box: 'border-box' });
      }
      refreshPromptHeight();
    }

    /** Write where each bar belongs. Every bar reaches it through its CSS
     *  transition on `translate` (styles/mobile.css), so this runs only when
     *  the answer may have changed, never to animate. */
    function applyTransform() {
      const onThreadPane = currentViewKey === 'thread';
      const offset = headerRef.current ? chromeOffsetPx({
        disabled,
        keyboardOpen,
        away: intent.away,
        scrollTop: viewScrollTop,
        headerHeight: cachedHeight,
        titleBarHeight: onThreadPane ? titleBarHeight : 0,
      }) : 0;
      // Read before any style write below, so the reads force no style flush.
      // Only an away header off the thread pane needs the transcript's position.
      // With no transcript there, both bars read it as at rest and stay shown.
      let threadScrollTop = viewScrollTop;
      let threadDistanceToBottom = distanceToBottomPx;
      if (!onThreadPane && offset < 0) {
        threadScrollTop = threadScroller ? clampedScrollTop(threadScroller) : 0;
        threadDistanceToBottom = threadScroller ? distanceToBottom(threadScroller, threadScrollTop) : 0;
      }
      if (headerRef.current) {
        const titleOffsetRem = titleBarOffsetPx({
          onThreadPane,
          chromeOffset: offset,
          threadScrollTop,
          headerHeight: cachedHeight,
          titleBarHeight,
        }) / cachedRemSize;
        // The header moves by its own height at most. On the thread pane the
        // title bar's extra height reaches only the bar's own offset.
        const headerTranslate = Math.max(-cachedHeight, offset);
        headerRef.current.style.translate = headerTranslate !== 0
          ? `0 ${headerTranslate / cachedRemSize}rem` : '';
        // Guarded to avoid needless style invalidation.
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
        // Never a layout property, and on the two consumers only.
        if (promptEl) promptEl.style.translate = promptOffsetRem ? `0 ${promptOffsetRem}rem` : '';
        downChevronEl?.style.setProperty('--mobile-prompt-offset', `${promptOffsetRem}rem`);
      }
    }

    /** The keyboard closed: the reader was just typing, so every bar returns. */
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
      // padding-top is var(--safe-area-top) (mobile.css). Any change
      // to it also changes the bounding height, so we only re-read when the
      // height delta above already proved something moved — skipping a forced
      // style flush on every no-op refreshHeight tick.
      cachedSafeAreaTop = parseFloat(getComputedStyle(el).paddingTop) || 0;
      updateHeaderVar();
      // An away header moves by its own height, so a header that grows while
      // away would otherwise peek in by the growth.
      applyTransform();
    }

    /** The container's offset, with iOS elastic bounce at either end clamped
     *  away so it cannot move the header. */
    function clampedScrollTop(container: Element): number {
      const maxScroll = Math.max(0, container.scrollHeight - container.clientHeight);
      return Math.min(Math.max(0, container.scrollTop), maxScroll);
    }

    function onScroll() {
      const header = headerRef.current;
      if (!header || cachedHeight === 0 || !currentContainer || disabled) return;

      recoverKeyboardState();

      // The iOS compositor-recovery nudge (utils/webkitRepaint.ts) writes ±1px to
      // this exact container and puts it back a frame later. Both writes fire a
      // real scroll event, and turning them into header deltas made the header
      // twitch by a pixel once per nudge. On a streaming thread that nudge runs
      // on a ~200ms throttle, so the header shook continuously while the user was
      // doing nothing at all (reported on an iOS PWA, 2026-08-03).
      //
      // Neither leg is harmless. The nudge leg alone already moves the header for
      // a frame, and at the clamp the pair does not even cancel: with the header
      // fully visible the reveal leg clamps at 0 while the restore leg is free to
      // hide, so the header settles a pixel low and then oscillates 0 to -1px on
      // every nudge after that.
      //
      // Skip the event WITHOUT advancing prevScrollTop: the nudge returns to the
      // value it came from, so the pre-nudge baseline stays correct and any real
      // scroll racing the nudge is simply folded into the next event's delta.
      // ContentPane's repaint call site has carried "do not reintroduce a
      // navigation-triggered repaint here without first making scroll consumers
      // ignore the repaint nudge" since that collision was first traced; this is
      // the scroll consumer doing the ignoring.
      //
      // A live drag overrides the window, because the two gates are duals and
      // must stay that way: forceWebKitRepaint refuses to WRITE a nudge while
      // isUserScrolling() (it would cancel iOS momentum), so a nudge can only
      // exist when the user is still. Suppressing during a drag could therefore
      // only ever eat the user's own events, and `lastNudgeAt` is module-global,
      // so a repaint of a different pane must not be able to freeze this one's
      // header. isUserScrolling() keys off `touchmove`, never `scroll`, so the
      // nudge's own synthetic event cannot trip this bypass. The residual case is
      // a drag beginning in the frame between a nudge and its restore, which
      // costs the 1px twitch back for one frame, invisible against the finger's
      // own motion and far cheaper than dropping real scroll deltas.
      if (isRepaintNudging() && !isUserScrolling()) return;

      const scrollTop = clampedScrollTop(currentContainer);

      // Within a pixel of where our last write left us, which the 1px
      // repaint nudge is allowed to spend. Anywhere else the reader has really
      // moved, so the stamp is spent and cannot mute a later reveal.
      const atRebasedTop = rebasedTop >= 0 && Math.abs(scrollTop - rebasedTop) <= 1;
      if (!atRebasedTop) rebasedTop = -1;

      // One of OUR OWN navigations is writing scrollTop frame by frame (a
      // chevron tap, turn-nav, a deep-link glide). Those scroll events are not
      // the user reading, so reset the header to visible instead of hiding it on
      // the way down. Same for the window right after a deep-link lands
      // (isHeaderPinnedForScroll): .chat-exchange's scroll-margin-top is sized
      // for the visible-header case, so a half-hidden header would leave the
      // landed event partly covered.
      // An ANCHOR write is the exception, and it is the opposite request: the
      // app moved the container precisely so the reader's line would NOT move.
      // Revealing the chrome there covers that line with the header they had
      // scrolled away, by up to a header plus a thread title. So the intent is
      // kept and only the baseline re-taken, which is what makes the correction
      // invisible instead of a jump. See `markAnchorScroll`.
      // `isAnchorScroll` reads module state at EVENT time. Any later mark
      // overwrites it, so a late anchor event arrives dressed as a placement
      // and takes the reveal. The POSITION answers it exactly. An event that
      // finds the container where our last write left it is that write's own,
      // and the write already decided the bars (`onRebasedScroll` below).
      // The follow's CARRY holds the bars too, through the same kind check and
      // position stamp. A reply scrolling the thread is not the reader, and
      // only their finger moves the bars (ADR 0337). The prompt shares every
      // rule here.
      const readerScroll = countsAsReaderScroll({
        navigation: isNavigationScroll(),
        headerPinned: isHeaderPinnedForScroll(),
      });
      if (!readerScroll) {
        prevScrollTop = scrollTop;
        measureContainer(currentContainer);
        if (!isAnchorScroll() && !isCarryScroll() && !atRebasedTop) revealBars();
        applyTransform();
        return;
      }

      // iOS Safari doesn't always blur inputs when swiping between scroll-snap panes.
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

    /** iOS Safari sometimes misses focusout — when swiping scroll-snap panes,
     *  or when the focused input is removed mid-edit. syncToScroll re-applies
     *  the offsets; updateHeaderVar alone leaves the header stuck a header
     *  height above the viewport. */
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

      // Check before early return — runs on every pane switch AND
      // every MutationObserver callback, even when container is unchanged.
      recoverKeyboardState();

      // A pane swipe brings every bar back. It is written BEFORE the rebind
      // below, so the outgoing up chevron glides in as it leaves.
      if (container !== currentContainer) {
        revealBars();
        applyTransform();
      }

      // Also before the early return: the title bar and chevron can be REPLACED
      // while the scroll container itself is reused (switching threads reuses
      // .thread-content). Now that the offset is written on those two elements
      // rather than inherited from the root, a missed rebind leaves the new
      // element with no value at all, so the title bar would sit at its resting
      // position while the header is scrolled away.
      const rebound = bindOffsetConsumers(container);
      bindPrompt();

      if (container === currentContainer) {
        // Nothing else changed, but freshly-bound elements need the current
        // offset written to them now. The container-change path below reaches
        // its own applyTransform.
        if (rebound) applyTransform();
        return;
      }

      if (currentContainer) {
        currentContainer.removeEventListener('scroll', onScroll);
      }

      currentContainer = container;
      currentContainerPane = container?.closest('.mobile-swipe-pane') ?? null;
      currentViewKey = view;
      // Stamped against the container we just left, so it says nothing here.
      rebasedTop = -1;
      refreshHeight();
      if (container) {
        measureContainer(container);
        prevScrollTop = viewScrollTop;
        container.addEventListener('scroll', onScroll, { passive: true });
      } else {
        // applyTransform still hides the header if the keyboard is open.
        prevScrollTop = 0;
        viewScrollTop = 0;
        distanceToBottomPx = Infinity;
      }
      applyTransform();
    }

    refreshHeight();
    attachListener();

    // Hide header and collapse spacer when any prompt input gains focus.
    // applyTransform checks keyboardOpen independently of the intent,
    // so this can't race with attachListener or scroll updates.
    function onFocusIn(e: FocusEvent) {
      if (!opensSoftwareKeyboard(e.target)) return;
      const target = e.target as HTMLElement;
      if (headerRef.current?.contains(target)) return;
      // Title bar is inside the scroll pane (not the header) but should
      // behave like a header input — don't hide when editing the title.
      if (target.closest('.mobile-thread-title-row')) return;
      // Header pinned visible (pinned bars / app-ui): it never slides off for
      // the keyboard, so the spacer must stay full-height. Collapsing it here
      // would slide content up behind the still-visible header (editing a
      // device name on an iOS PWA rendered the input under the header).
      if (disabled) return;
      if (keyboardOpen) return; // Already hidden — skip duplicate scroll compensation
      keyboardOpen = true;
      updateHeaderVar();
      // Compensate scroll: spacer shrinks from cachedHeight to cachedSafeAreaTop,
      // so move content up by the same delta to stay visually anchored.
      if (currentContainer) {
        const delta = cachedHeight - cachedSafeAreaTop;
        holdAcrossRelayout(currentContainer as HTMLElement, Math.max(0, currentContainer.scrollTop - delta));
      }
      applyTransform();
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
      if (keepsHeaderAway(e.relatedTarget as Element | null)) return;
      // After the press that blurred, so its release still hits its target.
      // Until then `recoverKeyboardState` leaves the close to this, which also
      // compensates the scroll.
      closePending = true;
      afterPressSettles(() => {
        closePending = false;
        if (disposed || !keyboardOpen || keepsHeaderAway(document.activeElement)) return;
        keyboardOpen = false;
        updateHeaderVar();
        // Mirror onFocusIn: spacer grows back, so add the same delta to scrollTop.
        // This runs after a send's two landing calls, so an unmarked write
        // would read as the reader and cancel the landing.
        if (currentContainer) {
          holdAcrossRelayout(currentContainer as HTMLElement, currentContainer.scrollTop + cachedHeight - cachedSafeAreaTop);
        }
        syncToScroll(currentContainer);
      });
    }
    function keepsHeaderAway(el: Element | null): boolean {
      return !!el && opensSoftwareKeyboard(el) && !headerRef.current?.contains(el)
        && !el.closest('.mobile-thread-title-row');
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
        // Content can shrink under a still reader (steps collapsed, a session
        // finished), bringing the top or the live edge within reach. Re-measure,
        // so the edge rules show the bars there.
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

    // Reveal the header and the prompt on request: a change applied, discarded
    // or reverted, a deep link landing, or the prompt about to take focus. The
    // reader is often scrolled far down with both bars away.
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
        // Gates the prompt's overlay layout in styles/mobile.css. Off, the
        // prompt keeps its in-flow place and the transcript no spacer.
        document.documentElement.toggleAttribute('data-mobile-dynamic-bars', dynamicBars);
        updatePromptHeightVar();
      }
      const next = shouldKeepHeaderVisible({
        view: mobileView.value,
        overlayType: panelOverlay.value?.type,
        dynamicBars,
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

    // The app wrote this container's offset. Re-take the baseline HERE, not on
    // the scroll event: on WebKit that event can arrive after the navigation
    // window has closed, handing the header the whole jump. A thread opening
    // at its saved place is the largest such jump. See `onRebasedScroll`.
    //
    // A placement or a held write that moved the container takes the reader
    // somewhere, so it reveals the bars at the write. An anchor write and the
    // follow's carry leave the intent untouched: only the reader's finger
    // moves the bars.
    //
    // And AGAIN on the next frame, because the write is not the end of it. A
    // reveal that shrinks the transcript is still settling, and the browser's
    // own clamp lands after. That clamp is a scroll nobody asked for, and the
    // header spent it as a reveal of the full title and bar.
    // The baseline alone only settles the DELTA path. Stamp the position too,
    // so the navigation path can recognise this write's own event.
    const unsubRebased = onRebasedScroll((el, kind) => {
      if (el !== currentContainer) return;
      const top = clampedScrollTop(el);
      // The 1px slack is the repaint nudge's, as at `atRebasedTop`.
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
      recomputeDisabled();
    });

    const unsubOverlay = panelOverlay.subscribe(recomputeDisabled);
    const unsubPrefs = preferences.subscribe(recomputeDisabled);
    // Entering or leaving the compose-empty view: its prompt never slides, and
    // the thread a first send lands on starts with the prompt shown.
    const unsubCompose = composeViewActive.subscribe(() => {
      revealBars();
      applyTransform();
    });

    return () => {
      disposed = true;
      if (currentContainer) {
        currentContainer.removeEventListener('scroll', onScroll);
      }
      document.removeEventListener('focusin', onFocusIn);
      document.removeEventListener('focusout', onFocusOut);
      document.removeEventListener('reveal-mobile-bars', onRevealBars);
      window.removeEventListener('resize', refreshHeight);
      window.visualViewport?.removeEventListener('resize', refreshHeight);
      if (coldStartPollId !== null) cancelAnimationFrame(coldStartPollId);
      if (mutationRafId !== null) cancelAnimationFrame(mutationRafId);
      if (rebaseSettleRaf !== null) cancelAnimationFrame(rebaseSettleRaf);
      observer.disconnect();
      headerResizeObserver.disconnect();
      if (titleBarResizeObserver) titleBarResizeObserver.disconnect();
      unsubRebased();
      unsub();
      unsubOverlay();
      unsubPrefs();
      unsubCompose();
      promptResizeObserver?.disconnect();
      if (promptEl) promptEl.style.translate = '';
      downChevronEl?.style.removeProperty('--mobile-prompt-offset');
      document.documentElement.removeAttribute('data-mobile-dynamic-bars');
      document.documentElement.style.removeProperty('--mobile-prompt-height');
      if (headerRef.current) {
        headerRef.current.style.translate = '';
      }
      document.documentElement.style.removeProperty('--mobile-header-height');
      document.documentElement.style.removeProperty('--mobile-thread-title-height');
      // The offsets live on their consumers, not the root (bindOffsetConsumers).
      titleBarEl?.style.removeProperty('--mobile-header-offset');
      chevronEl?.style.removeProperty('--mobile-header-offset');
    };
  }, [mobileLayout]);
}
