/**
 * The Lucidos tooltip: one implementation for the host shell and for app iframes.
 *
 * The host calls it through the `useTooltip` hook. The SDK bundle calls it on
 * load inside an app iframe, so `data-tooltip` works in an app with no init
 * call. The attribute contract is documented in `system-knowhow/js-sdk.md`.
 *
 * The CSS half lives in the host's `styles/global/shared-components.css`, which
 * the engine appends to the served `/api/v1/sdk-iframe.css`. Both halves have
 * one copy, pinned by `styles/__tests__/tooltip-single-source.test.ts`.
 *
 * A document stands the layer down with `data-lucidos-tooltips="off"` on
 * `<html>` or `<body>`, or by calling `lucidos.ui.disableTooltips()`. It also
 * stands down whenever the page owns a `#tooltip` node of its own.
 */

import { clampToViewportX } from './geometry';

/** Pure decision: is the tooltip text redundant against what the user sees?
 *  When the tooltip repeats the element's visible text and that text is fully
 *  visible (not CSS-truncated), it adds nothing, so the layer suppresses it.
 *  Truncated text keeps the tooltip, so mobile tap-to-reveal still works for
 *  long titles and file names. */
export function isRedundantTooltip(visibleText: string, tooltipText: string, isTruncated: boolean): boolean {
  if (isTruncated) return false;
  return tooltipText.trim().toLowerCase() === visibleText.trim().toLowerCase();
}

/** Touch movement past this many pixels is treated as a swipe/scroll, not a tap. */
const TOUCH_SWIPE_THRESHOLD_PX = 10;

/** Hold this long (without swiping) to reveal a tooltip on touch. Tuned to the
 *  platform long-press convention. */
const LONG_PRESS_MS = 450;

/** Hover this long before a tooltip appears, so a pointer crossing a toolbar
 *  does not trail one behind it. */
const HOVER_DELAY_MS = 300;

/** Did the finger travel far enough between touchstart and the current point
 *  that we should treat the gesture as a swipe (not a tap)? */
export function isTouchSwipe(startX: number, startY: number, currentX: number, currentY: number): boolean {
  return Math.hypot(currentX - startX, currentY - startY) > TOUCH_SWIPE_THRESHOLD_PX;
}

/** Where the tooltip's arrow should point.
 *
 *  By default the tooltip anchors to the element's BORDER: horizontally
 *  centered, vertically at the top or bottom edge. So it always sits outside
 *  the element and points at it, wherever the pointer entered. It does not
 *  drift as the pointer moves inside. That holds for a tall element too, such
 *  as a wrapped-title drawer row.
 *
 *  Elements that set `data-tooltip-follow-cursor` track the cursor instead.
 *  The full-height split divider is the one caller: a border anchor would
 *  fling its tooltip to the far end of the pane, away from the pointer. */
export function computeTooltipAnchor(
  rect: { left: number; width: number; top: number; bottom: number },
  mouseX: number,
  mouseY: number,
  followCursor: boolean,
): { anchorX: number; anchorTop: number; anchorBottom: number } {
  return {
    anchorX: followCursor ? mouseX : rect.left + rect.width / 2,
    anchorTop: followCursor ? mouseY : rect.top,
    anchorBottom: followCursor ? mouseY : rect.bottom,
  };
}

/** Breathing room (px) between the tooltip and its target, and between the
 *  tooltip's top edge and the unsafe top inset. */
export const TOOLTIP_GAP_PX = 8;

/** Decide the tooltip's vertical placement. Prefer ABOVE the anchor. Flip to
 *  BELOW when placing above would push the tooltip's top into the unsafe top
 *  inset. `safeTop` is env(safe-area-inset-top): the iOS status bar, notch or
 *  Dynamic Island, and 0 on devices without one. `forceBelow`
 *  (data-tooltip-below) always places below.
 *
 *  Returns the viewport `top`, and whether the tooltip ended up above, which
 *  drives the CSS arrow flip. On a notchless device the threshold collapses
 *  back to the plain gap. */
export function computeTooltipVerticalPlacement(
  anchorTop: number,
  anchorBottom: number,
  tooltipHeight: number,
  safeTop: number,
  forceBelow: boolean,
  gap = TOOLTIP_GAP_PX,
): { top: number; above: boolean } {
  if (!forceBelow) {
    const aboveTop = anchorTop - tooltipHeight - gap;
    if (aboveTop >= safeTop + gap) return { top: aboveTop, above: true };
  }
  return { top: anchorBottom + gap, above: false };
}

/** Compute a new anchor point so the tooltip stays glued to its target after a
 *  scroll. The offset is captured at show time, relative to the target's
 *  top-left. */
export function reanchorToTarget(
  rect: { left: number; top: number },
  offset: { x: number; y: number },
): { x: number; y: number } {
  return { x: rect.left + offset.x, y: rect.top + offset.y };
}

/** Does this element carry tooltip content: plain `data-tooltip` text, or a
 *  structured `data-tooltip-rows` grid? Both forms anchor the layer. */
function hasTooltipContent(el: HTMLElement): boolean {
  return el.hasAttribute('data-tooltip') || el.hasAttribute('data-tooltip-rows');
}

function shouldSuppress(target: HTMLElement): boolean {
  // Structured-row tooltips never just repeat the visible row text, so the
  // redundancy check (which only compares plain `data-tooltip`) never applies.
  if (target.hasAttribute('data-tooltip-rows')) return false;
  const text = target.getAttribute('data-tooltip');
  if (!text) return false;
  const visible = target.textContent || '';
  const truncated = target.scrollWidth > target.clientWidth || target.scrollHeight > target.clientHeight;
  return isRedundantTooltip(visible, text, truncated);
}

/** A normalized structured-tooltip row: label, value, and an optional status
 *  tone keyword that paints a leading dot in the value cell. */
export interface ParsedTooltipRow {
  label: string;
  value: string;
  tone?: string;
}

/** Parse a `data-tooltip-rows` JSON payload into normalized rows. Pure (no
 *  DOM), so it is unit-testable. A malformed or non-array payload yields an
 *  empty list, and the caller then renders nothing. */
export function parseTooltipRows(rowsJson: string): ParsedTooltipRow[] {
  let parsed: unknown;
  try {
    parsed = JSON.parse(rowsJson);
  } catch {
    // The payload is always a caller's own `JSON.stringify(rows)`, so a parse
    // failure is a structurally-impossible programmer error. It is not a
    // runtime condition the user could act on, and a hover-time error toast
    // would be absurd. An empty tooltip is the only sensible outcome, so we
    // swallow this deliberately rather than surfacing it.
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  return parsed.map((r) => {
    const row = r as { label?: unknown; value?: unknown; tone?: unknown };
    return {
      label: String(row.label ?? ''),
      value: String(row.value ?? ''),
      ...(typeof row.tone === 'string' ? { tone: row.tone } : {}),
    };
  });
}

/** Build the two-column label/value DOM rows for a `data-tooltip-rows` tooltip,
 *  ready to mount into `#tooltip-text`. */
function buildTooltipRows(rowsJson: string): HTMLElement[] {
  return parseTooltipRows(rowsJson).map((r) => {
    const row = document.createElement('div');
    row.className = 'tt-row';
    const label = document.createElement('span');
    label.className = 'tt-label';
    label.textContent = r.label;
    const value = document.createElement('span');
    value.className = 'tt-value';
    if (r.tone) {
      const dot = document.createElement('span');
      dot.className = `tt-dot tt-dot-${r.tone}`;
      value.appendChild(dot);
    }
    value.appendChild(document.createTextNode(r.value));
    row.appendChild(label);
    row.appendChild(value);
    return row;
  });
}

/** The markup opt-out. Set it to `off` on `<html>` or `<body>`. */
export const TOOLTIPS_ATTR = 'data-lucidos-tooltips';

/** Pure: does either host element carry the opt-out? */
export function isTooltipsOptedOut(htmlValue: string | null, bodyValue: string | null): boolean {
  return htmlValue === 'off' || bodyValue === 'off';
}

/** Pure: does the page own a `#tooltip` node this layer did not create? Apps in
 *  the wild hand-roll one, and two tooltips on one screen is worse than none,
 *  so the layer stands down. `ours` is null before the first hover, so any node
 *  found then is foreign. */
export function hasForeignTooltip(found: readonly Element[], ours: Element | null): boolean {
  return found.some((el) => el !== ours);
}

export interface TooltipOptions {
  /** Require `data-tooltip-longpress` before a touch long press reveals.
   *  True in the host shell, where a plain tappable row must not be hijacked.
   *  False in an app iframe, where any `data-tooltip` answers a long press. */
  longPressNeedsOptIn?: boolean;
  /** Hide this many ms after the finger lifts off a long press. Null keeps the
   *  tooltip up until the next tap, which is what the host shell does. */
  hideAfterLongPressMs?: number | null;
}

/** Stand-down callbacks, one per live install. `disableTooltips()` runs them. */
const liveInstalls = new Set<() => void>();

/** The documented opt-out, for an app that ships its own tooltip. It sets the
 *  markup attribute, which the check gating every show already reads, and drops
 *  any node this layer has built. */
export function disableTooltips(): void {
  document.documentElement.setAttribute(TOOLTIPS_ATTR, 'off');
  for (const standDown of liveInstalls) standDown();
}

/**
 * Install the tooltip layer on this document, and return a cleanup function.
 *
 * Delegated: it listens on `document`, so an element added later is covered
 * with no re-scan and no per-element wiring.
 */
export function installTooltips(options: TooltipOptions = {}): () => void {
  const longPressNeedsOptIn = options.longPressNeedsOptIn ?? true;
  const hideAfterLongPressMs = options.hideAfterLongPressMs ?? null;

  let tipEl: HTMLDivElement | null = null;
  let arrowEl: HTMLDivElement | null = null;
  let titleEl: HTMLDivElement | null = null;
  let textEl: HTMLDivElement | null = null;
  // Hidden probe whose padding-top is env(safe-area-inset-top). JS cannot read
  // env() directly, so we measure the notch inset off this element.
  let safeAreaProbe: HTMLDivElement | null = null;
  let showTimer: number | null = null;
  let releaseTimer: number | null = null;
  let currentTarget: HTMLElement | null = null;
  // Anchor offset relative to the target's top-left at show time, so we can
  // re-position to the same spot after the page (or container) scrolls.
  let anchorOffsetX = 0;
  let anchorOffsetY = 0;

  /** Should this document get no tooltip at all right now?
   *
   *  Checked before every show, never once at install. An app builds its own
   *  `#tooltip` from a script at the end of `<body>`. That runs after `sdk.js`
   *  in `<head>`, so an install-time check would look too early to see it. */
  function isStoodDown(): boolean {
    const body = document.body as HTMLElement | null;
    const optedOut = isTooltipsOptedOut(
      document.documentElement.getAttribute(TOOLTIPS_ATTR),
      body ? body.getAttribute(TOOLTIPS_ATTR) : null,
    );
    if (optedOut) return true;
    return hasForeignTooltip(Array.from(document.querySelectorAll('#tooltip')), tipEl);
  }

  function ensureEl() {
    if (tipEl) return;
    tipEl = document.createElement('div');
    tipEl.id = 'tooltip';
    arrowEl = document.createElement('div');
    arrowEl.id = 'tooltip-arrow';
    titleEl = document.createElement('div');
    titleEl.id = 'tooltip-title';
    textEl = document.createElement('div');
    textEl.id = 'tooltip-text';
    tipEl.appendChild(arrowEl);
    tipEl.appendChild(titleEl);
    tipEl.appendChild(textEl);
    document.body.appendChild(tipEl);

    safeAreaProbe = document.createElement('div');
    safeAreaProbe.setAttribute('aria-hidden', 'true');
    safeAreaProbe.style.cssText =
      'position:fixed;top:0;left:0;width:0;height:0;visibility:hidden;pointer-events:none;padding-top:env(safe-area-inset-top,0px);';
    document.body.appendChild(safeAreaProbe);
  }

  /** Drop this install's nodes. The listeners stay, and the show-time check
   *  keeps them inert while the opt-out attribute is set. */
  function removeNodes() {
    if (tipEl?.parentNode) tipEl.parentNode.removeChild(tipEl);
    if (safeAreaProbe?.parentNode) safeAreaProbe.parentNode.removeChild(safeAreaProbe);
    tipEl = null;
    arrowEl = null;
    titleEl = null;
    textEl = null;
    safeAreaProbe = null;
  }

  /** env(safe-area-inset-top) in px, measured off the hidden probe. It reads 0
   *  on a device without a notch, and inside an app iframe. */
  function readSafeAreaTop(): number {
    if (!safeAreaProbe) return 0;
    return parseFloat(getComputedStyle(safeAreaProbe).paddingTop) || 0;
  }

  function isVisible(): boolean {
    return !!tipEl && tipEl.style.opacity === '1';
  }

  function position(target: HTMLElement, mouseX: number, mouseY: number) {
    ensureEl();
    const rowsJson = target.getAttribute('data-tooltip-rows');
    const text = target.getAttribute('data-tooltip');
    if ((!rowsJson && !text) || !tipEl || !arrowEl || !titleEl || !textEl) return;

    const title = target.getAttribute('data-tooltip-title') || '';
    titleEl.textContent = title;
    if (rowsJson !== null) {
      textEl.classList.add('tooltip-rows');
      textEl.replaceChildren(...buildTooltipRows(rowsJson));
    } else {
      textEl.classList.remove('tooltip-rows');
      textEl.textContent = text;
    }

    // Skip the show-time opacity dance when we are re-positioning a tooltip
    // already on screen, such as a mouse move or a scroll-follow. Toggling
    // opacity 0 to 1 each scroll frame would flicker on a slow device.
    const wasVisible = isVisible();
    if (!wasVisible) {
      tipEl.style.display = 'block';
      tipEl.style.opacity = '0';
    }
    tipEl.classList.remove('above');

    const tipRect = tipEl.getBoundingClientRect();
    const targetRect = target.getBoundingClientRect();

    // Anchor to the element's border by default, so the tooltip sits outside
    // even a tall wrapped-title row. Only data-tooltip-follow-cursor elements
    // track the pointer. See computeTooltipAnchor.
    const followCursor = target.hasAttribute('data-tooltip-follow-cursor');
    const { anchorX, anchorTop, anchorBottom } = computeTooltipAnchor(targetRect, mouseX, mouseY, followCursor);

    // Vertical: prefer above, flip below when above would collide with the
    // unsafe top inset. So a header title's tooltip never renders up behind the
    // camera strip on iOS. data-tooltip-below forces below.
    const forceBelow = target.hasAttribute('data-tooltip-below');
    const { top, above } = computeTooltipVerticalPlacement(
      anchorTop, anchorBottom, tipRect.height, readSafeAreaTop(), forceBelow,
    );

    // Horizontal: center on the anchor, clamp to the viewport.
    const left = clampToViewportX(anchorX - tipRect.width / 2, tipRect.width);

    tipEl.style.top = `${top}px`;
    tipEl.style.left = `${left}px`;
    if (!wasVisible) tipEl.style.opacity = '1';
    tipEl.classList.toggle('above', above);

    // Arrow: point at the anchor X, clamped within the tooltip's own bounds.
    const arrowX = Math.max(10, Math.min(anchorX - left, tipRect.width - 10));
    arrowEl.style.left = `${arrowX}px`;
  }

  /** Reveal the tooltip, and report whether anything appeared. A stood-down
   *  layer answers false, so no caller can claim a gesture it did not serve. */
  function show(target: HTMLElement, mouseX: number, mouseY: number): boolean {
    if (isStoodDown()) return false;
    const targetRect = target.getBoundingClientRect();
    anchorOffsetX = mouseX - targetRect.left;
    anchorOffsetY = mouseY - targetRect.top;
    currentTarget = target;
    position(target, mouseX, mouseY);
    return true;
  }

  function hide() {
    if (showTimer) { clearTimeout(showTimer); showTimer = null; }
    if (releaseTimer) { clearTimeout(releaseTimer); releaseTimer = null; }
    if (tipEl) { tipEl.style.opacity = '0'; tipEl.style.display = 'none'; }
    currentTarget = null;
  }

  function findTarget(el: EventTarget | null): HTMLElement | null {
    let node = el as HTMLElement | null;
    while (node && node !== document.body) {
      if (hasTooltipContent(node)) return node;
      node = node.parentElement;
    }
    return null;
  }

  const isTouchDevice = 'ontouchstart' in window;

  function onOver(e: MouseEvent) {
    // On a touch device the touch handlers own the tooltip. A mouseover there
    // is always synthetic, fired after a touch, and it painted phantom
    // tooltips whenever a drawer or overlay closed.
    if (isTouchDevice) return;
    const target = findTarget(e.target);
    if (!target || !hasTooltipContent(target)) {
      if (currentTarget) hide();
      return;
    }
    if (target === currentTarget) return;

    hide();
    currentTarget = target;
    showTimer = window.setTimeout(() => {
      if (currentTarget !== target) return;
      // Clear currentTarget when suppressing, so a later mouseout does not try
      // to hide a tooltip we never showed.
      if (shouldSuppress(target)) { currentTarget = null; return; }
      show(target, e.clientX, e.clientY);
    }, HOVER_DELAY_MS);
  }

  function onMove(e: MouseEvent) {
    if (isTouchDevice) return;
    if (!currentTarget) return;
    const target = findTarget(e.target);
    if (target !== currentTarget) { hide(); return; }
    if (isVisible()) show(currentTarget, e.clientX, e.clientY);
  }

  function onOut(e: MouseEvent) {
    if (isTouchDevice) return;
    const from = findTarget(e.target);
    const to = findTarget(e.relatedTarget);
    if (from === currentTarget && to !== currentTarget) hide();
  }

  // Mouse-only dismissal. Touch dismissal happens in onTouchEnd, so we can tell
  // a tap from a swipe and avoid flashing the tooltip mid-swipe.
  function onMouseDown() {
    if (isTouchDevice) return;
    if (currentTarget) hide();
  }

  // Losing focus hides. Inside an app iframe this is the only signal that the
  // user clicked host chrome, because no mouseout arrives in the frame.
  function onBlur() {
    if (currentTarget) hide();
  }

  // Keep an ALREADY VISIBLE tooltip glued to its target as the page, or any
  // scroll container, scrolls. Capture phase, so nested scrollers count too.
  // Skip when only the hover timer has armed currentTarget: a scroll would
  // otherwise reveal a tooltip that has not shown yet.
  function onScroll() {
    if (!currentTarget || !isVisible()) return;
    const rect = currentTarget.getBoundingClientRect();
    const { x, y } = reanchorToTarget(rect, { x: anchorOffsetX, y: anchorOffsetY });
    position(currentTarget, x, y);
  }

  let touchStartX = 0;
  let touchStartY = 0;
  let touchMoved = false;
  let longPressTimer: number | null = null;
  let longPressFired = false;

  function clearLongPress() {
    if (longPressTimer) { clearTimeout(longPressTimer); longPressTimer = null; }
  }

  // After a long press reveals a tooltip, the gesture's terminating tap still
  // dispatches a `click`, which would activate whatever sits under the finger.
  // So swallow the next click at the document capture phase, ahead of every
  // bubble-phase handler. It self-disarms when no click arrives, which is what
  // some browsers do after a long touch.
  function armClickSwallow() {
    const swallow = (ev: Event) => {
      ev.stopPropagation();
      ev.preventDefault();
      document.removeEventListener('click', swallow, true);
      clearTimeout(disarm);
    };
    document.addEventListener('click', swallow, true);
    const disarm = window.setTimeout(() => document.removeEventListener('click', swallow, true), 700);
  }

  function onTouchStart(e: TouchEvent) {
    const touch = e.touches[0];
    touchStartX = touch.clientX;
    touchStartY = touch.clientY;
    touchMoved = false;
    longPressFired = false;
    clearLongPress();
    if (releaseTimer) { clearTimeout(releaseTimer); releaseTimer = null; }

    // Long-press reveal, the touch counterpart of desktop hover. The host shell
    // requires data-tooltip-longpress, so a plain tappable row is never
    // hijacked. An app iframe answers any data-tooltip.
    const target = findTarget(e.target);
    const eligible = !longPressNeedsOptIn || !!target?.hasAttribute('data-tooltip-longpress');
    if (target && eligible && !shouldSuppress(target)) {
      const x = touch.clientX;
      const y = touch.clientY;
      longPressTimer = window.setTimeout(() => {
        longPressTimer = null;
        if (touchMoved) return; // became a scroll or swipe, not a long press
        // Claim the gesture only when a tooltip actually appeared. A stood-down
        // layer shows nothing, so swallowing the click that ends the press
        // would kill the button under the finger for no visible reason.
        if (!show(target, x, y)) return;
        longPressFired = true;
        armClickSwallow();
      }, LONG_PRESS_MS);
    }
  }

  function onTouchMove(e: TouchEvent) {
    if (touchMoved) return;
    const touch = e.touches[0];
    if (isTouchSwipe(touchStartX, touchStartY, touch.clientX, touch.clientY)) {
      touchMoved = true;
      clearLongPress(); // a swipe cancels the pending long-press reveal
      // A swipe (pane navigation, scroll) dismisses a tooltip already revealed
      // by tap or long press. Otherwise it stays stuck over a target that just
      // swiped away. We never preventDefault, so the swipe still navigates.
      if (isVisible()) hide();
    }
  }

  function onTouchEnd(e: TouchEvent) {
    const wasLongPress = longPressFired;
    longPressFired = false;
    clearLongPress();

    // The release that ENDS a long press keeps the tooltip up. It then clears
    // itself after hideAfterLongPressMs, or waits for the next tap when that is
    // null. Re-arm the swallow here: the one armed at reveal time is fused from
    // the reveal, so a press held past that fuse has already dropped it. The
    // click this release produces arrives now, so arm it now.
    if (wasLongPress) {
      armClickSwallow();
      if (hideAfterLongPressMs !== null && isVisible()) {
        releaseTimer = window.setTimeout(hide, hideAfterLongPressMs);
      }
      return;
    }

    if (touchMoved) return; // Swipe, not tap. Ignore.

    // A tap while a tooltip shows dismisses it, and swallows that same tap so
    // it does not ALSO activate what sits under the finger. The reveal was an
    // explicit long press, so the next tap means "close this".
    //
    // Two swallows, because targets activate on different events. The
    // stopPropagation() here runs at the document capture phase, ahead of the
    // target's own bubble-phase touchend handler. That kills the buttons which
    // act on touchend and then cancel the synthetic click. armClickSwallow()
    // catches that click for the rest, such as plain rows and links.
    if (currentTarget) { e.stopPropagation(); hide(); armClickSwallow(); return; }

    // Elements with data-tooltip-tap opt into tap-to-show on touch devices.
    const target = findTarget(e.target);
    if (!target?.hasAttribute('data-tooltip-tap')) return;
    if (shouldSuppress(target)) return;
    const touch = e.changedTouches[0];
    show(target, touch.clientX, touch.clientY);
  }

  // Passive on scroll and touch, so a document-level listener can never block
  // the start of a mobile scroll. None of these handlers call preventDefault().
  const passiveCapture = { capture: true, passive: true };
  document.addEventListener('mouseover', onOver, true);
  document.addEventListener('mousemove', onMove, true);
  document.addEventListener('mouseout', onOut, true);
  document.addEventListener('mousedown', onMouseDown, true);
  document.addEventListener('scroll', onScroll, passiveCapture);
  document.addEventListener('touchstart', onTouchStart, passiveCapture);
  document.addEventListener('touchmove', onTouchMove, passiveCapture);
  document.addEventListener('touchend', onTouchEnd, passiveCapture);
  window.addEventListener('blur', onBlur);

  const standDown = () => { hide(); removeNodes(); };
  liveInstalls.add(standDown);

  return () => {
    liveInstalls.delete(standDown);
    document.removeEventListener('mouseover', onOver, true);
    document.removeEventListener('mousemove', onMove, true);
    document.removeEventListener('mouseout', onOut, true);
    document.removeEventListener('mousedown', onMouseDown, true);
    document.removeEventListener('scroll', onScroll, passiveCapture);
    document.removeEventListener('touchstart', onTouchStart, passiveCapture);
    document.removeEventListener('touchmove', onTouchMove, passiveCapture);
    document.removeEventListener('touchend', onTouchEnd, passiveCapture);
    window.removeEventListener('blur', onBlur);
    // Cancel any pending reveal before the nodes go. The hover timer calls
    // show(), which re-creates #tooltip and re-appends it to <body> after this
    // teardown removed it. That leaks an orphan node no cleanup owns.
    // clearLongPress() covers the touch counterpart.
    if (showTimer) { clearTimeout(showTimer); showTimer = null; }
    if (releaseTimer) { clearTimeout(releaseTimer); releaseTimer = null; }
    clearLongPress();
    removeNodes();
  };
}
