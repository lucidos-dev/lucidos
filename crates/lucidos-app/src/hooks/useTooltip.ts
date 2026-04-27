import { useEffect } from 'preact/hooks';
import { clampToViewportX } from '../utils/dom';

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

    function position(target: HTMLElement, mouseX: number, mouseY: number) {
      ensureEl();
      const text = target.getAttribute('data-tooltip');
      if (!text || !tipEl || !arrowEl || !titleEl || !textEl) return;

      const title = target.getAttribute('data-tooltip-title') || '';
      titleEl.textContent = title;
      textEl.textContent = text;

      // Make visible but transparent so we can measure
      tipEl.style.display = 'block';
      tipEl.style.opacity = '0';
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
      tipEl.style.opacity = '1';
      tipEl.classList.toggle('above', above);

      // Arrow: point at mouse X, clamped within tooltip bounds
      const arrowX = Math.max(10, Math.min(mouseX - left, tipRect.width - 10));
      arrowEl.style.left = `${arrowX}px`;
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
        if (currentTarget === target) position(target, e.clientX, e.clientY);
      }, 800);
    }

    function onMove(e: MouseEvent) {
      if (!currentTarget) return;
      const target = findTarget(e.target);
      if (target !== currentTarget) { hide(); return; }
      // Update position if already visible
      if (tipEl && tipEl.style.opacity === '1') {
        position(currentTarget, e.clientX, e.clientY);
      }
    }

    function onOut(e: MouseEvent) {
      const from = findTarget(e.target);
      const to = findTarget(e.relatedTarget);
      if (from === currentTarget && to !== currentTarget) hide();
    }

    // Also hide on scroll and click
    function onDismiss() { if (currentTarget) hide(); }

    function onTouchStart(e: TouchEvent) {
      // Dismiss any visible tooltip on tap
      if (currentTarget) { hide(); return; }

      // Elements with data-tooltip-tap opt into tap-to-show on touch devices
      const target = findTarget(e.target);
      if (target?.hasAttribute('data-tooltip-tap')) {
        const touch = e.touches[0];
        currentTarget = target;
        position(target, touch.clientX, touch.clientY);
      }
    }

    function onTouchMove() {
      if (currentTarget) hide();
    }

    document.addEventListener('mouseover', onOver, true);
    document.addEventListener('mousemove', onMove, true);
    document.addEventListener('mouseout', onOut, true);
    document.addEventListener('scroll', onDismiss, true);
    document.addEventListener('mousedown', onDismiss, true);
    document.addEventListener('touchstart', onTouchStart, true);
    document.addEventListener('touchmove', onTouchMove, true);

    return () => {
      document.removeEventListener('mouseover', onOver, true);
      document.removeEventListener('mousemove', onMove, true);
      document.removeEventListener('mouseout', onOut, true);
      document.removeEventListener('scroll', onDismiss, true);
      document.removeEventListener('mousedown', onDismiss, true);
      document.removeEventListener('touchstart', onTouchStart, true);
      document.removeEventListener('touchmove', onTouchMove, true);
      if (tipEl?.parentNode) tipEl.parentNode.removeChild(tipEl);
    };
  }, []);
}
