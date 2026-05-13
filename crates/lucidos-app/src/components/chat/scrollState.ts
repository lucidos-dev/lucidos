import { signal } from '@preact/signals';

/** Shared scroll-position signals for the chat area.
 *  ThreadView and CreateThreadView are mounted twice each (desktop SplitLayout
 *  + mobile MobileSwipeContainer), so writers MUST gate on isElementVisible(el)
 *  before mutating these signals — see makeScrollObservers below. The hidden
 *  duplicate's element has 0×0 dimensions and would otherwise overwrite the
 *  visible instance's correct values (e.g. clearing notAtTop because the hidden
 *  el reports isScrollable=false).
 *
 *  Two thresholds, two purposes:
 *  - `scrolledUp` uses an 80px stickiness window: while inside it, content
 *    growth during streaming still auto-scrolls to bottom and keyboard/header
 *    flows treat the user as bottom-pinned. Crossing the window means the
 *    user has chosen to read history, so auto-scroll backs off.
 *  - `awayFromBottom` flips on the very first pixel of scroll-up so the
 *    scroll-to-bottom chevron appears immediately, independent of stickiness. */
export const scrolledUp = signal(false);
export const awayFromBottom = signal(false);
export const notAtTop = signal(false);

/** The currently-active scroll container element.
 *
 *  Set by useAutoScroll when it attaches listeners to a new element.
 *  Used by scrollToBottom() instead of document.querySelector('.thread-content')
 *  which is fragile — on mobile both desktop (hidden) and mobile scroll containers
 *  exist in the DOM, and querySelector finds the hidden one first.
 *
 *  Design decision: this is a plain mutable variable, not a signal, because
 *  nothing needs to react to it changing — it's only read imperatively by
 *  scrollToBottom(). */
let _activeScrollElement: HTMLElement | null = null;

export function setActiveScrollElement(el: HTMLElement | null) {
  _activeScrollElement = el;
}

/** True if the element is actually visible — not hidden via display:none
 *  AND not clipped by a zero-height ancestor with overflow:hidden (e.g.
 *  mobile .content-row which collapses to height:0 instead of display:none
 *  so that position:fixed children like ThreadDrawer still render). */
export function isElementVisible(el: HTMLElement): boolean {
  const r = el.getBoundingClientRect();
  if (r.width <= 0 || r.height <= 0) return false;
  // Walk up to detect clipping ancestors — an element inside a zero-height
  // overflow:hidden container reports non-zero dimensions from layout but
  // is visually invisible.
  let ancestor = el.parentElement;
  while (ancestor && ancestor !== document.documentElement) {
    // display:contents removes the element's box — getBoundingClientRect()
    // returns 0×0 but children are fully visible. Skip these ancestors.
    if (getComputedStyle(ancestor).display === 'contents') {
      ancestor = ancestor.parentElement;
      continue;
    }
    const ar = ancestor.getBoundingClientRect();
    if (ar.height <= 0 || ar.width <= 0) return false;
    ancestor = ancestor.parentElement;
  }
  return true;
}

/** Fallback for when _activeScrollElement hasn't been set yet.
 *  Uses visibility check to skip hidden duplicates on mobile. */
function findVisibleThreadContent(): HTMLElement | null {
  if (typeof document === 'undefined' || !document.querySelectorAll) return null;
  const elements = document.querySelectorAll('.thread-content');
  for (const el of elements) {
    if (isElementVisible(el as HTMLElement)) return el as HTMLElement;
  }
  return null;
}

export function getActiveScrollElement(): HTMLElement | null {
  return _activeScrollElement;
}

/** Suppression mode for ResizeObserver during scroll-to-bottom.
 *
 *  'scroll' — actively scroll to bottom on each resize (content still rendering)
 *  'ignore' — do nothing (suppression expired, normal mode)
 *
 *  Race condition without this: scrollToBottom() scrolls to current bottom,
 *  then new content renders (pending user message), scrollHeight grows,
 *  ResizeObserver fires and sees isAtBottom()===false → sets scrolledUp=true
 *  → auto-scroll effect skips → user never sees the bottom.
 *
 *  Uses time-based window (300ms) instead of rAF counting because mobile
 *  devices render content over many more frames than desktop. */
let _resizeMode: 'scroll' | 'ignore' = 'ignore';
let _suppressTimer: ReturnType<typeof setTimeout> | null = null;
const SUPPRESSION_MS = 500;

/** Get current resize mode — 'scroll' means ResizeObserver should scroll
 *  to bottom instead of setting scrolledUp. */
export function getResizeMode() {
  return _resizeMode;
}

/** Extend the suppression window — called from ResizeObserver when in 'scroll'
 *  mode to keep the window alive while content is still rendering. */
export function extendSuppression() {
  if (_suppressTimer) clearTimeout(_suppressTimer);
  _suppressTimer = setTimeout(() => {
    _resizeMode = 'ignore';
    _suppressTimer = null;
  }, SUPPRESSION_MS);
}

/** Resolve the visible scroll container — re-checks on each call so
 *  layout switches (desktop ↔ mobile) mid-animation don't scroll a stale element. */
function resolveTarget(): HTMLElement | null {
  let el = _activeScrollElement;
  if (el && !isElementVisible(el)) el = null;
  return el ?? findVisibleThreadContent();
}

/** Active scroll loop timer — cleared when a new scrollToBottom() call
 *  starts so only the latest invocation drives the loop. Uses setTimeout
 *  (~16ms) because iOS Safari can silently no-op scrollTo(options) during
 *  viewport transitions — direct scrollTop assignment is more reliable. */
let _scrollTimer: ReturnType<typeof setTimeout> | null = null;

/** Reset scrolledUp, immediately scroll the response area to the bottom,
 *  and keep scrolling at frame rate until the suppression window expires.
 *
 *  Called from PromptInput.submit() and sendMessage() — any place where
 *  we KNOW the user wants to be at the bottom and new content is about
 *  to render.
 *
 *  iOS Safari PWA keyboard animations take 300-400ms with many
 *  visualViewport.resize events. The old 2×rAF approach missed most of
 *  the animation. Now we scroll every ~16ms for the full 500ms
 *  suppression window, re-reading scrollHeight each time so layout
 *  changes (keyboard close, content render) are always caught. */
export function scrollToBottom() {
  scrolledUp.value = false;
  awayFromBottom.value = false;
  _resizeMode = 'scroll';

  // Immediate scroll
  const target = resolveTarget();
  if (target) {
    target.scrollTop = target.scrollHeight;
  }

  // Cancel any prior loop so only the latest call drives scrolling
  if (_scrollTimer !== null) clearTimeout(_scrollTimer);

  // Continuous scroll loop — runs every ~16ms until suppression expires.
  // Re-resolves target each frame in case the visible element changed.
  const loop = () => {
    if (_resizeMode !== 'scroll') {
      // The loop clears awayFromBottom every iteration, so a final-frame
      // content grow without an onScroll/onResize would leave the chevron
      // stuck hidden. Reconcile against actual position on exit.
      const el = resolveTarget();
      if (el && el.scrollTop + el.clientHeight < el.scrollHeight - 2) {
        awayFromBottom.value = true;
      }
      _scrollTimer = null;
      return;
    }
    scrolledUp.value = false;
    awayFromBottom.value = false;
    const el = resolveTarget();
    if (el) {
      el.scrollTop = el.scrollHeight;
    }
    _scrollTimer = setTimeout(loop, 16);
  };
  _scrollTimer = setTimeout(loop, 16);

  extendSuppression();
}

/** Re-pin the user to the bottom across a layout shift the user just caused —
 *  typing in the prompt (action buttons grow), clicking a question option
 *  (card transitions to the answered state), clicking a permission button
 *  (card grows the picked/rejected styling). If they were at the bottom,
 *  engage scroll mode so the upcoming ResizeObserver fire is suppressed; if
 *  they had scrolled up, no-op so we don't override their intent.
 *
 *  Without this, onResize sees the post-shift layout cross the 80px window
 *  and flips scrolledUp=true, breaking auto-scroll for subsequent streaming. */
export function preserveAtBottom() {
  if (!scrolledUp.value) scrollToBottom();
}

/** Build the scroll- and resize-event handlers for a single .thread-content
 *  element. The visibility gate at the top of each handler is required by the
 *  dual-mounting contract documented at the top of this file. */
export function makeScrollObservers(el: HTMLElement) {
  function isAtBottom() {
    return el.scrollTop + el.clientHeight >= el.scrollHeight - 80;
  }
  // Tighter threshold for the chevron — flips on the first pixel of
  // scroll-up. The 2px slack absorbs subpixel rounding (mobile zoom,
  // device-pixel snapping) without making the chevron look stuck.
  function isVisuallyAtBottom() {
    return el.scrollTop + el.clientHeight >= el.scrollHeight - 2;
  }
  function isAtTop() {
    return el.scrollTop <= 80;
  }
  function isScrollable() {
    return el.scrollHeight > el.clientHeight + 10;
  }
  function syncNotAtTop() {
    notAtTop.value = isScrollable() && !isAtTop();
  }
  // Scroll events: can both set and clear scrolledUp (user gesture or
  // programmatic scrollTop assignment — both produce real scroll events).
  // Skip during suppression ('scroll' mode) — scroll events in this window
  // are from programmatic scrollToBottom() or header scroll compensation
  // (useHideOnScroll adjusts scrollTop on focusin/focusout), not user intent.
  // Without this guard, iOS keyboard dismiss causes: focusout → scrollTop
  // compensation → scroll event → scrolledUp=true → viewport resize handler
  // skips scrollToBottom() → user loses bottom-pinned state.
  //
  // awayFromBottom is updated unconditionally on scroll — programmatic
  // scrolls leave the container at the bottom, so the check returns false
  // and the chevron hides naturally without a special-case branch.
  // (Content growth that doesn't fire a scroll event is handled by
  // useEffect snapping back to bottom; see onResize for the shrink case.)
  function onScroll() {
    if (!isElementVisible(el)) return;
    syncNotAtTop();
    awayFromBottom.value = !isVisuallyAtBottom();
    if (getResizeMode() === 'scroll') return; // only scrolledUp is suppressed
    scrolledUp.value = !isAtBottom();
  }
  // Resize events: behavior depends on resize mode set by scrollToBottom().
  //
  // 'scroll' mode: content is rendering after a scrollToBottom() call —
  //   actively scroll to bottom on each resize and extend the suppression
  //   window. This keeps us pinned to the bottom as content progressively
  //   renders (especially important on mobile where rendering is slow).
  //
  // 'ignore' mode (normal): can only *escalate* scrolledUp to true.
  //   Must NEVER clear scrolledUp — otherwise a layout change (textarea shrink
  //   after submit, idle banner removal) can falsely reset scrolledUp and
  //   trigger unwanted auto-scroll.
  function onResize() {
    if (!isElementVisible(el)) return;
    // Sync on resize too — if content shrinks below the viewport,
    // clear the chevron even if no scroll event fires.
    syncNotAtTop();
    if (getResizeMode() === 'scroll') {
      el.scrollTop = el.scrollHeight;
      extendSuppression();
      return;
    }
    // Gate on the 80px window (see top of file) so streaming tokens don't
    // trip the chevron before useEffect snaps back. Larger growth (panel
    // expand, multi-line code block) crosses the window and the chevron
    // appears immediately, even though no scroll event fired.
    if (!isAtBottom()) {
      scrolledUp.value = true;
      awayFromBottom.value = true;
    }
    // Clear path: if content shrinks so the user is now visually at the
    // bottom (idle banner removed, step collapsed), hide the chevron
    // without waiting for a scroll event.
    if (awayFromBottom.value && isVisuallyAtBottom()) {
      awayFromBottom.value = false;
    }
  }
  return { onScroll, onResize };
}
