import { signal } from '@preact/signals';
import { isElementVisible } from '../../utils/elementVisibility';
import { transcriptScrolls } from './transcriptScrolls';

export const awayFromBottom = signal(false);

export const notAtTop = signal(false);

export const scrolledFromTop = signal(false);

let _activeScrollElement: HTMLElement | null = null;

export function setActiveScrollElement(el: HTMLElement | null) {
  _activeScrollElement = el;
}

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

export function resolveTarget(): HTMLElement | null {
  let el = _activeScrollElement;
  if (el && !isElementVisible(el)) el = null;
  return el ?? findVisibleThreadContent();
}

export function liveEdgeTop(el: HTMLElement): number {
  return Math.max(0, el.scrollHeight - el.clientHeight);
}

export function clampToScrollRange(el: HTMLElement, top: number): number {
  return Math.max(0, Math.min(top, liveEdgeTop(el)));
}

export const RESTING_SLACK_PX = 2;

export function isLiveEdgeTop(el: HTMLElement, top: number): boolean {
  return top >= liveEdgeTop(el) - RESTING_SLACK_PX;
}

export function isAtLiveEdge(el: HTMLElement): boolean {
  return isLiveEdgeTop(el, el.scrollTop);
}

export function isScrollable(el: HTMLElement): boolean {
  return transcriptScrolls(el);
}

const TURN_LANDING_FALLBACK_GAP_PX = 8;

export function turnLandingClearancePx(turn: HTMLElement): number {
  if (typeof getComputedStyle !== 'function') return TURN_LANDING_FALLBACK_GAP_PX;
  const px = parseFloat(getComputedStyle(turn).scrollMarginTop);
  return Number.isFinite(px) && px > 0 ? px : TURN_LANDING_FALLBACK_GAP_PX;
}

export function syncAwayFromBottom() {
  const el = resolveTarget();
  if (!el) return;
  awayFromBottom.value = !isAtLiveEdge(el);
}
