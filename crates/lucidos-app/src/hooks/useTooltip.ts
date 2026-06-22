import { useEffect } from 'preact/hooks';
import { clampToViewportX } from '../utils/dom';

/** Pure decision: is the tooltip text redundant against what the user already
 *  sees? When the tooltip just repeats the element's visible text and that
 *  text is fully visible (not CSS-truncated), there's nothing to add — the
 *  global system suppresses it. Truncated text keeps the tooltip so mobile
 *  tap-to-reveal still works for long titles, file names, etc. */
export function isRedundantTooltip(visibleText: string, tooltipText: string, isTruncated: boolean): boolean {
  if (isTruncated) return false;
  return tooltipText.trim().toLowerCase() === visibleText.trim().toLowerCase();
}

/** Touch movement past this many pixels is treated as a swipe/scroll, not a tap. */
const TOUCH_SWIPE_THRESHOLD_PX = 10;

/** Hold this long (without swiping) on a `data-tooltip-longpress` element to
 *  reveal its tooltip on touch. Tuned to the platform long-press convention. */
const LONG_PRESS_MS = 450;

/** Did the finger travel far enough between touchstart and the current point
 *  that we should treat the gesture as a swipe (not a tap)? */
export function isTouchSwipe(startX: number, startY: number, currentX: number, currentY: number): boolean {
  return Math.hypot(currentX - startX, currentY - startY) > TOUCH_SWIPE_THRESHOLD_PX;
}

/** Where the tooltip's arrow should point. By default the tooltip anchors to the
 *  *element's border* — horizontally centered, vertically at the top/bottom edge —
 *  so it always sits fully OUTSIDE the element, pointing up (tooltip below) or down
 *  (tooltip above) at it, no matter where the pointer entered and without drifting
 *  as the pointer moves within it. This holds even for a TALL element such as a
 *  wrapped-title thread drawer row: a height-based "follow the cursor" heuristic
 *  used to flip any element over ~100px into pointer-tracking, which dropped the
 *  tooltip *inside* the row and covered it.
 *
 *  Only elements that opt in via `data-tooltip-follow-cursor` (the full-height
 *  split divider, where a border anchor would fling the tooltip to the far end of
 *  the pane, away from the pointer) track the cursor instead. */
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

/** Compute a new anchor point so the tooltip stays glued to the same spot on
 *  its target after the page (or any scroll container) has scrolled. The
 *  offset is captured at show-time relative to the target's top-left. */
export function reanchorToTarget(
  rect: { left: number; top: number },
  offset: { x: number; y: number },
): { x: number; y: number } {
  return { x: rect.left + offset.x, y: rect.top + offset.y };
}

/** Does this element carry tooltip content — plain `data-tooltip` text OR a
 *  structured `data-tooltip-rows` grid? Both forms anchor the global system. */
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

/** Parse a `data-tooltip-rows` JSON payload into normalized rows. Pure (no DOM)
 *  so it's unit-testable; a malformed or non-array payload yields an empty list,
 *  and the caller then renders nothing. */
export function parseTooltipRows(rowsJson: string): ParsedTooltipRow[] {
  let parsed: unknown;
  try {
    parsed = JSON.parse(rowsJson);
  } catch {
    // The payload is always this app's own `JSON.stringify(TooltipRow[])`, so a
    // parse failure is a structurally-impossible programmer error, not a runtime
    // condition the user could act on. Degrading to an empty tooltip is the
    // correct (and only sensible) outcome — a hover-time error toast would be
    // absurd UX — so we swallow it deliberately rather than surfacing it.
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

/** Build the structured two-column label/value DOM rows for a `data-tooltip-rows`
 *  tooltip, ready to mount into `#tooltip-text`. */
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

/**
 * Global tooltip system using event delegation.
 * Replaces CSS ::after tooltips to support mouse-centered positioning
 * and consistent styling regardless of parent element styles.
 */
export function useTooltip() {
  useEffect(() => {
    let tipEl: HTMLDivElement | null = null;
    let arrowEl: HTMLDivElement | null = null;
    let titleEl: HTMLDivElement | null = null;
    let textEl: HTMLDivElement | null = null;
    let showTimer: number | null = null;
    let currentTarget: HTMLElement | null = null;
    // Anchor offset relative to the target's top-left at show time, so we can
    // re-position to the same spot after the page (or container) scrolls.
    let anchorOffsetX = 0;
    let anchorOffsetY = 0;

    function ensureEl() {
      if (!tipEl) {
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
      }
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

      // Skip the show-time opacity dance if we're just re-positioning a
      // tooltip that's already on screen (e.g. mouse-move, scroll-follow):
      // toggling opacity 0→1 each scroll frame would flicker on slow devices.
      const wasVisible = isVisible();
      if (!wasVisible) {
        tipEl.style.display = 'block';
        tipEl.style.opacity = '0';
      }
      tipEl.classList.remove('above');

      const tipRect = tipEl.getBoundingClientRect();
      const targetRect = target.getBoundingClientRect();
      const gap = 8;

      // Anchor to the element's border by default (so the tooltip always sits
      // outside the element, even a tall wrapped-title drawer row); only elements
      // that opt in via data-tooltip-follow-cursor (the split divider) track the
      // pointer — see computeTooltipAnchor.
      const followCursor = target.hasAttribute('data-tooltip-follow-cursor');
      const { anchorX, anchorTop, anchorBottom } = computeTooltipAnchor(targetRect, mouseX, mouseY, followCursor);

      // Vertical: prefer above, fall back to below.
      // data-tooltip-below forces below (useful for elements at top of viewport).
      const forceBelow = target.hasAttribute('data-tooltip-below');
      let top: number;
      let above: boolean;
      if (forceBelow) {
        top = anchorBottom + gap;
        above = false;
      } else {
        top = anchorTop - tipRect.height - gap;
        above = true;
        if (top < 8) {
          top = anchorBottom + gap;
          above = false;
        }
      }

      // Horizontal: center on the anchor, clamp to viewport.
      const left = clampToViewportX(anchorX - tipRect.width / 2, tipRect.width);

      tipEl.style.top = `${top}px`;
      tipEl.style.left = `${left}px`;
      if (!wasVisible) tipEl.style.opacity = '1';
      tipEl.classList.toggle('above', above);

      // Arrow: point at the anchor X, clamped within tooltip bounds.
      const arrowX = Math.max(10, Math.min(anchorX - left, tipRect.width - 10));
      arrowEl.style.left = `${arrowX}px`;
    }

    function show(target: HTMLElement, mouseX: number, mouseY: number) {
      const targetRect = target.getBoundingClientRect();
      anchorOffsetX = mouseX - targetRect.left;
      anchorOffsetY = mouseY - targetRect.top;
      currentTarget = target;
      position(target, mouseX, mouseY);
    }

    function hide() {
      if (showTimer) { clearTimeout(showTimer); showTimer = null; }
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
      // On touch devices, tooltips are handled entirely via onTouchStart (tap).
      // Mouseover events on mobile are always synthetic (fired after touch) and
      // cause phantom tooltips when drawers/overlays close.
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
        // Clear currentTarget when suppressing so a later mouseout doesn't
        // try to hide() a tooltip we never showed.
        if (shouldSuppress(target)) { currentTarget = null; return; }
        show(target, e.clientX, e.clientY);
      }, 300);
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

    // Mouse-only dismissal. Touch dismissal happens in onTouchEnd so we can
    // distinguish taps from swipes and avoid flashing the tooltip mid-swipe.
    function onMouseDown() {
      if (isTouchDevice) return;
      if (currentTarget) hide();
    }

    // Keep an *already visible* tooltip glued to its target as the page (or
    // any scroll container) scrolls. Capture-phase so we catch nested
    // scrollers too. Skip when only the hover timer has armed currentTarget
    // — otherwise scroll would reveal a tooltip that hasn't shown yet.
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

    // After a long-press reveals a tooltip, the gesture's terminating tap still
    // dispatches a `click` — which would activate whatever is under the finger
    // (e.g. open the thread). Swallow the next click at the document capture
    // phase, before any bubble-phase handler runs. Mirrors the Overlay
    // paired-swallow pattern; self-disarms if no click arrives (some browsers
    // suppress the click after a long touch).
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

      // Long-press reveal: the touch counterpart of desktop hover. Opt-in via
      // data-tooltip-longpress so it never hijacks a plain tappable row.
      const target = findTarget(e.target);
      if (target?.hasAttribute('data-tooltip-longpress') && !shouldSuppress(target)) {
        const x = touch.clientX;
        const y = touch.clientY;
        longPressTimer = window.setTimeout(() => {
          longPressTimer = null;
          if (touchMoved) return; // became a scroll/swipe — not a long press
          longPressFired = true;
          show(target, x, y);
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
      }
    }

    function onTouchEnd(e: TouchEvent) {
      const wasLongPress = longPressFired;
      longPressFired = false;
      clearLongPress();

      // The release that ENDS a long-press must keep the just-revealed tooltip
      // visible (the click-swallow is already armed); a later tap dismisses it.
      if (wasLongPress) return;

      if (touchMoved) return; // Swipe, not tap — ignore.

      // Tap while a tooltip is showing dismisses it — and, like a modal/overlay
      // dismiss, swallows the same tap so it doesn't ALSO activate whatever sits
      // under the finger (open the thread, fire a row action). The tooltip was
      // revealed by an explicit long-press, so the next tap means "close this",
      // not "close it AND press what's behind it".
      //
      // Two swallows are needed because targets activate on different events:
      //   - stopPropagation() here (we run at the document CAPTURE phase, before
      //     the target's own bubble-phase onTouchEnd) kills buttons that act on
      //     touchend and preventDefault() the synthetic click — everything using
      //     composeHandlers (the compose icon), which a click-only swallow misses.
      //   - armClickSwallow() catches the synthetic click for the remaining,
      //     click-activated targets (plain thread rows, links).
      if (currentTarget) { e.stopPropagation(); hide(); armClickSwallow(); return; }

      // Elements with data-tooltip-tap opt into tap-to-show on touch devices.
      const target = findTarget(e.target);
      if (!target?.hasAttribute('data-tooltip-tap')) return;
      if (shouldSuppress(target)) return;
      const touch = e.changedTouches[0];
      show(target, touch.clientX, touch.clientY);
    }

    // Passive on scroll/touch so a global document-level listener can't block
    // mobile scroll start. None of these handlers call preventDefault().
    const passiveCapture = { capture: true, passive: true };
    document.addEventListener('mouseover', onOver, true);
    document.addEventListener('mousemove', onMove, true);
    document.addEventListener('mouseout', onOut, true);
    document.addEventListener('mousedown', onMouseDown, true);
    document.addEventListener('scroll', onScroll, passiveCapture);
    document.addEventListener('touchstart', onTouchStart, passiveCapture);
    document.addEventListener('touchmove', onTouchMove, passiveCapture);
    document.addEventListener('touchend', onTouchEnd, passiveCapture);

    return () => {
      document.removeEventListener('mouseover', onOver, true);
      document.removeEventListener('mousemove', onMove, true);
      document.removeEventListener('mouseout', onOut, true);
      document.removeEventListener('mousedown', onMouseDown, true);
      document.removeEventListener('scroll', onScroll, passiveCapture);
      document.removeEventListener('touchstart', onTouchStart, passiveCapture);
      document.removeEventListener('touchmove', onTouchMove, passiveCapture);
      document.removeEventListener('touchend', onTouchEnd, passiveCapture);
      if (tipEl?.parentNode) tipEl.parentNode.removeChild(tipEl);
    };
  }, []);
}
