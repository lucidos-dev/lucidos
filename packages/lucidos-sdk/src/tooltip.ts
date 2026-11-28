
import { clampToViewportX } from './geometry';

export function isRedundantTooltip(visibleText: string, tooltipText: string, isTruncated: boolean): boolean {
  if (isTruncated) return false;
  return tooltipText.trim().toLowerCase() === visibleText.trim().toLowerCase();
}

const TOUCH_SWIPE_THRESHOLD_PX = 10;

const LONG_PRESS_MS = 450;

export const TOOLTIP_HOVER_DELAY_MS = 300;

export function isTouchSwipe(startX: number, startY: number, currentX: number, currentY: number): boolean {
  return Math.hypot(currentX - startX, currentY - startY) > TOUCH_SWIPE_THRESHOLD_PX;
}

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

export const TOOLTIP_GAP_PX = 8;

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

export function reanchorToTarget(
  rect: { left: number; top: number },
  offset: { x: number; y: number },
): { x: number; y: number } {
  return { x: rect.left + offset.x, y: rect.top + offset.y };
}

function hasTooltipContent(el: HTMLElement): boolean {
  return el.hasAttribute('data-tooltip') || el.hasAttribute('data-tooltip-rows');
}

function shouldSuppress(target: HTMLElement): boolean {
  if (target.hasAttribute('aria-haspopup') && target.getAttribute('aria-expanded') === 'true') return true;
  if (target.hasAttribute('data-tooltip-rows')) return false;
  const text = target.getAttribute('data-tooltip');
  if (!text) return false;
  const visible = target.textContent || '';
  const truncated = target.scrollWidth > target.clientWidth || target.scrollHeight > target.clientHeight;
  return isRedundantTooltip(visible, text, truncated);
}

export interface ParsedTooltipRow {
  label: string;
  value: string;
  tone?: string;
}

export function parseTooltipRows(rowsJson: string): ParsedTooltipRow[] {
  let parsed: unknown;
  try {
    parsed = JSON.parse(rowsJson);
  } catch {
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

export const TOOLTIP_SHORTCUT_SEPARATOR = '\u001f';

export function splitTooltipShortcut(value: string): { text: string; shortcut: string | null } {
  const at = value.lastIndexOf(TOOLTIP_SHORTCUT_SEPARATOR);
  if (at < 0) return { text: value, shortcut: null };
  return { text: value.slice(0, at), shortcut: value.slice(at + TOOLTIP_SHORTCUT_SEPARATOR.length) };
}

function buildShortcutRow(shortcut: string): HTMLElement {
  const row = document.createElement('div');
  row.className = 'tooltip-shortcut';
  const key = document.createElement('kbd');
  key.textContent = shortcut;
  row.appendChild(key);
  return row;
}

export const TOOLTIPS_ATTR = 'data-lucidos-tooltips';

export function isTooltipsOptedOut(htmlValue: string | null, bodyValue: string | null): boolean {
  return htmlValue === 'off' || bodyValue === 'off';
}

export function hasForeignTooltip(found: readonly Element[], ours: Element | null): boolean {
  return found.some((el) => el !== ours);
}

export interface TooltipOptions {
  longPressNeedsOptIn?: boolean;
  hideAfterLongPressMs?: number | null;
}

const liveInstalls = new Set<() => void>();

export function disableTooltips(): void {
  document.documentElement.setAttribute(TOOLTIPS_ATTR, 'off');
  for (const standDown of liveInstalls) standDown();
}

export function installTooltips(options: TooltipOptions = {}): () => void {
  const longPressNeedsOptIn = options.longPressNeedsOptIn ?? true;
  const hideAfterLongPressMs = options.hideAfterLongPressMs ?? null;

  let tipEl: HTMLDivElement | null = null;
  let arrowEl: HTMLDivElement | null = null;
  let titleEl: HTMLDivElement | null = null;
  let textEl: HTMLDivElement | null = null;
  let safeAreaProbe: HTMLDivElement | null = null;
  let showTimer: number | null = null;
  let releaseTimer: number | null = null;
  let currentTarget: HTMLElement | null = null;
  let anchorOffsetX = 0;
  let anchorOffsetY = 0;

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
      'position:fixed;top:0;left:0;width:0;height:0;visibility:hidden;pointer-events:none;'
      + 'padding-top:var(--safe-area-top, env(safe-area-inset-top,0px));';
    document.body.appendChild(safeAreaProbe);
  }

  function removeNodes() {
    if (tipEl?.parentNode) tipEl.parentNode.removeChild(tipEl);
    if (safeAreaProbe?.parentNode) safeAreaProbe.parentNode.removeChild(safeAreaProbe);
    tipEl = null;
    arrowEl = null;
    titleEl = null;
    textEl = null;
    safeAreaProbe = null;
  }

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
    } else if (text !== null) {
      textEl.classList.remove('tooltip-rows');
      const { text: body, shortcut } = splitTooltipShortcut(text);
      textEl.textContent = body;
      if (shortcut) textEl.appendChild(buildShortcutRow(shortcut));
    }

    const wasVisible = isVisible();
    if (!wasVisible) {
      tipEl.style.display = 'block';
      tipEl.style.opacity = '0';
    }
    tipEl.classList.remove('above');

    const tipRect = tipEl.getBoundingClientRect();
    const targetRect = target.getBoundingClientRect();

    const followCursor = target.hasAttribute('data-tooltip-follow-cursor');
    const { anchorX, anchorTop, anchorBottom } = computeTooltipAnchor(targetRect, mouseX, mouseY, followCursor);

    const forceBelow = target.hasAttribute('data-tooltip-below');
    const { top, above } = computeTooltipVerticalPlacement(
      anchorTop, anchorBottom, tipRect.height, readSafeAreaTop(), forceBelow,
    );

    const left = clampToViewportX(anchorX - tipRect.width / 2, tipRect.width);

    tipEl.style.top = `${top}px`;
    tipEl.style.left = `${left}px`;
    if (!wasVisible) tipEl.style.opacity = '1';
    tipEl.classList.toggle('above', above);

    const arrowX = Math.max(10, Math.min(anchorX - left, tipRect.width - 10));
    arrowEl.style.left = `${arrowX}px`;
  }

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
      if (shouldSuppress(target)) { currentTarget = null; return; }
      show(target, e.clientX, e.clientY);
    }, TOOLTIP_HOVER_DELAY_MS);
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

  function onPress() {
    if (isTouchDevice) return;
    if (currentTarget) hide();
  }

  function onBlur() {
    if (currentTarget) hide();
  }

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

    const target = findTarget(e.target);
    const eligible = !longPressNeedsOptIn || !!target?.hasAttribute('data-tooltip-longpress');
    if (target && eligible && !shouldSuppress(target)) {
      const x = touch.clientX;
      const y = touch.clientY;
      longPressTimer = window.setTimeout(() => {
        longPressTimer = null;
        if (touchMoved) return;
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
      clearLongPress();
      if (isVisible()) hide();
    }
  }

  function onTouchEnd(e: TouchEvent) {
    const wasLongPress = longPressFired;
    longPressFired = false;
    clearLongPress();

    if (wasLongPress) {
      armClickSwallow();
      if (hideAfterLongPressMs !== null && isVisible()) {
        releaseTimer = window.setTimeout(hide, hideAfterLongPressMs);
      }
      return;
    }

    if (touchMoved) return;

    if (currentTarget) { e.stopPropagation(); hide(); armClickSwallow(); return; }

    const target = findTarget(e.target);
    if (!target?.hasAttribute('data-tooltip-tap')) return;
    if (shouldSuppress(target)) return;
    const touch = e.changedTouches[0];
    show(target, touch.clientX, touch.clientY);
  }

  const passiveCapture = { capture: true, passive: true };
  document.addEventListener('mouseover', onOver, true);
  document.addEventListener('mousemove', onMove, true);
  document.addEventListener('mouseout', onOut, true);
  document.addEventListener('mousedown', onPress, true);
  document.addEventListener('click', onPress, true);
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
    document.removeEventListener('mousedown', onPress, true);
    document.removeEventListener('click', onPress, true);
    document.removeEventListener('scroll', onScroll, passiveCapture);
    document.removeEventListener('touchstart', onTouchStart, passiveCapture);
    document.removeEventListener('touchmove', onTouchMove, passiveCapture);
    document.removeEventListener('touchend', onTouchEnd, passiveCapture);
    window.removeEventListener('blur', onBlur);
    if (showTimer) { clearTimeout(showTimer); showTimer = null; }
    if (releaseTimer) { clearTimeout(releaseTimer); releaseTimer = null; }
    clearLongPress();
    removeNodes();
  };
}
