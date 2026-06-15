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

/** Compute a new anchor point so the tooltip stays glued to the same spot on
 *  its target after the page (or any scroll container) has scrolled. The
 *  offset is captured at show-time relative to the target's top-left. */
export function reanchorToTarget(
  rect: { left: number; top: number },
  offset: { x: number; y: number },
): { x: number; y: number } {
  return { x: rect.left + offset.x, y: rect.top + offset.y };
}

function shouldSuppress(target: HTMLElement): boolean {
  const text = target.getAttribute('data-tooltip');
  if (!text) return false;
  const visible = target.textContent || '';
  const truncated = target.scrollWidth > target.clientWidth || target.scrollHeight > target.clientHeight;
  return isRedundantTooltip(visible, text, truncated);
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
      const text = target.getAttribute('data-tooltip');
      if (!text || !tipEl || !arrowEl || !titleEl || !textEl) return;

      const title = target.getAttribute('data-tooltip-title') || '';
      titleEl.textContent = title;
      textEl.textContent = text;

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

      // For tall elements (like the split divider), position relative to
      // the mouse cursor instead of the element's top/bottom edge.
      const anchorTop = targetRect.height > 100 ? mouseY : targetRect.top;
      const anchorBottom = targetRect.height > 100 ? mouseY : targetRect.bottom;

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

      // Horizontal: center on mouse, clamp to viewport
      const left = clampToViewportX(mouseX - tipRect.width / 2, tipRect.width);

      tipEl.style.top = `${top}px`;
      tipEl.style.left = `${left}px`;
      if (!wasVisible) tipEl.style.opacity = '1';
      tipEl.classList.toggle('above', above);

      // Arrow: point at mouse X, clamped within tooltip bounds
      const arrowX = Math.max(10, Math.min(mouseX - left, tipRect.width - 10));
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
        if (node.hasAttribute && node.hasAttribute('data-tooltip')) return node;
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
      if (!target || !target.getAttribute('data-tooltip')) {
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

      // Tap on an already-visible tooltip dismisses it.
      if (currentTarget) { hide(); return; }

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
