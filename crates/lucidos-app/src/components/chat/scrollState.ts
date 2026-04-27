import { signal } from '@preact/signals';

/** Shared scroll-position signals for the chat area.
 *  Only one of CreateThreadView / ThreadView is mounted at a time (conditional render),
 *  so a single signal is correct — it tracks whichever is currently visible.
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
