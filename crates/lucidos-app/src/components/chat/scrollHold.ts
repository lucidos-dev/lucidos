import { anchorSpacer, setAnchorSpacer, splitAnchorCorrection } from './anchorCorrection';
import { isScrollHeld, onScrollHoldReleased, readerGestureSince, readerScrollOwed } from './readerInput';
import { forceWebKitRepaint, settledScrollTop } from '../../utils/webkitRepaint';
import { opensSoftwareKeyboard } from '../../utils/dom';
import { scaledDurationMs } from '../../utils/motion';
import { DISCLOSURE_MAX_MS } from '../../utils/disclosureMotion';
import { holdsReaderOn, holdPosition, isWhereWeLastScrolledIt, lastWriteKind, writeCount } from './scrollLedger';
import { isOtherNavigationScroll, markAnchorScroll, rideGlideInFlight, scrollAnimating } from './scrollNavigation';
import { followState, stopFollowingBottom } from './scrollRide';
import { clampToScrollRange, isAtLiveEdge } from './scrollSurface';
import { blockRelTop, readLineBlock, type LineBlock } from './scrollAnchor';
import { scrollPosition } from './scrollOwner';
import { nowMs } from '../../utils/scrollActivity';
import { isElementVisible } from '../../utils/elementVisibility';

function carryHeldScroll(el: HTMLElement): void {
  if (holdsReaderOn(el)) holdPosition(el);
}

const deferredHolds = new WeakMap<HTMLElement, { delta: number; writes: number }>();

onScrollHoldReleased((el) => {
  const hold = deferredHolds.get(el);
  deferredHolds.delete(el);
  if (!hold || hold.delta === 0 || writeCount(el) !== hold.writes || !isElementVisible(el)) return;
  holdAcrossRelayout(el, el.scrollTop + hold.delta);
  recordReadingLine(el);
});

export function holdAcrossRelayout(el: HTMLElement, top: number): void {
  if (isScrollHeld(el)) {
    const previous = deferredHolds.get(el);
    const writes = writeCount(el);
    const carried = previous?.writes === writes ? previous.delta : 0;
    deferredHolds.set(el, { delta: carried + (top - el.scrollTop), writes });
    return;
  }
  markAnchorScroll(el, top);
  carryHeldScroll(el);
}

export function honourAnchoredMutation(el: HTMLElement): void {
  recordReadingLine(el);
  carryHeldScroll(el);
  if (followState() === 'riding' && !isAtLiveEdge(el) && !rideGlideInFlight()) stopFollowingBottom();
}

type ReadingLine = Pick<LineBlock, 'block' | 'relTop' | 'turn' | 'turnRelTop'> & { recordedTop: number };

const readingLines = new WeakMap<HTMLElement, ReadingLine>();

export function rereadReadingLine(el: HTMLElement): boolean {
  const line = readLineBlock(el);
  if (line) readingLines.set(el, { ...line, recordedTop: el.scrollTop });
  return line !== null;
}

export function recordReadingLine(el: HTMLElement, target?: HTMLElement): void {
  if (rereadReadingLine(el)) return;
  const top = target ? blockRelTop(el, target) : null;
  if (target && top !== null) {
    readingLines.set(el, { block: target, relTop: top, turn: target, turnRelTop: top, recordedTop: el.scrollTop });
  } else {
    readingLines.delete(el);
  }
}

function shiftOfLine(el: HTMLElement, line: ReadingLine): number | null {
  const blockTop = blockRelTop(el, line.block);
  const top = blockTop ?? blockRelTop(el, line.turn);
  if (top === null) return null;
  return top - (blockTop === null ? line.turnRelTop : line.relTop);
}

const SETTLE_SCROLL_PX = 2;

export function recordReadingLineOnScroll(el: HTMLElement): void {
  const line = readingLines.get(el);
  const scrolled = line ? el.scrollTop - line.recordedTop : 0;
  const shift = line && Math.abs(scrolled) <= SETTLE_SCROLL_PX ? shiftOfLine(el, line) : null;
  const grown = shift === null ? 0 : shift + scrolled;
  if (Math.abs(grown) > 1 && holdReadingLine(el, false)) return;
  recordReadingLine(el);
}

export function holdReadingLine(el: HTMLElement, reflowed: boolean): boolean {
  const owner = scrollPosition.owner().kind;
  const rideStoodDown = reflowed && owner === 'ride' && scrollPosition.claim() !== null;
  if (owner !== 'reader' && !rideStoodDown) return false;
  if (scrollAnimating() || isScrollHeld(el) || readerScrollOwed(el) > 0) return false;

  const line = readingLines.get(el);
  const placed = lastWriteKind(el) === 'placement' && isWhereWeLastScrolledIt(el)
    && (el.scrollTop <= SETTLE_SCROLL_PX || !line || Math.abs(el.scrollTop - line.recordedTop) > SETTLE_SCROLL_PX);
  if (line && !placed) {
    const shift = shiftOfLine(el, line);
    if (shift !== null) {
      if (Math.abs(shift) <= 1) return true;
      const wanted = el.scrollTop + shift;
      const target = clampToScrollRange(el, wanted);
      if (Math.abs(target - el.scrollTop) > 1) holdAcrossRelayout(el, target);
      if (target === wanted) {
        line.recordedTop = el.scrollTop;
        return true;
      }
    }
  }
  recordReadingLine(el);
  return true;
}

function contentOffsetTop(container: HTMLElement, el: HTMLElement): number {
  return el.getBoundingClientRect().top - container.getBoundingClientRect().top + container.scrollTop;
}

const ANCHOR_SETTLE_FRAMES = 12;

const ROLL_SETTLE_SLACK_MS = 50;

export function withScrollAnchor(anchor: Element | null | undefined, fn: () => void) {
  const container = anchor?.closest('.thread-content') as HTMLElement | null;
  if (!container || !anchor || isScrollHeld(container)) { fn(); return; }

  const focused = document.activeElement;
  if (focused instanceof HTMLElement && container.contains(focused) && opensSoftwareKeyboard(focused)) {
    focused.blur();
  }

  const held = anchor as HTMLElement;
  const offsetBefore = contentOffsetTop(container, held);
  const scrollBefore = settledScrollTop(container);
  const overflowBefore = container.style.overflow;
  const pressedAt = nowMs();
  let restored = false;

  container.style.overflow = 'hidden';
  const offsetFrozen = contentOffsetTop(container, held);

  const restore = () => {
    if (restored) return;
    restored = true;
    observer.disconnect();
    container.style.overflow = overflowBefore;
    if (isScrollHeld(container)) return;

    const anchored = held.isConnected;
    const targetNow = (): { scrollTop: number; spacer: number } => {
      const offset = contentOffsetTop(container, held);
      return splitAnchorCorrection(scrollBefore + (offset - anchorSpacer(container) - offsetBefore));
    };
    const wanted = anchored ? targetNow() : { scrollTop: container.scrollTop, spacer: anchorSpacer(container) };
    setAnchorSpacer(container, wanted.spacer);
    holdAcrossRelayout(container, wanted.scrollTop);

    forceWebKitRepaint(container);

    honourAnchoredMutation(container);

    if (anchored) {
      let framesLeft = ANCHOR_SETTLE_FRAMES;
      const rollsLandAt = nowMs() + scaledDurationMs(DISCLOSURE_MAX_MS) + ROLL_SETTLE_SLACK_MS;
      let lastFrameAt = -Infinity;
      const reassert = (frameAt: number) => {
        if (!held.isConnected || isOtherNavigationScroll(container)) return;
        if (isScrollHeld(container) || readerGestureSince(container, pressedAt)) return;
        const target = targetNow();
        setAnchorSpacer(container, target.spacer);
        if (settledScrollTop(container) !== target.scrollTop) {
          holdAcrossRelayout(container, target.scrollTop);
          honourAnchoredMutation(container);
        }
        const rolling = frameAt > lastFrameAt && frameAt < rollsLandAt;
        lastFrameAt = frameAt;
        if (--framesLeft <= 0 && !rolling) return;
        requestAnimationFrame(reassert);
      };
      requestAnimationFrame(reassert);
    }
  };

  const observer = new MutationObserver(() => restore());
  observer.observe(container, { childList: true, subtree: true });

  fn();

  if (!restored && contentOffsetTop(container, held) !== offsetFrozen) restore();

  queueMicrotask(() => queueMicrotask(restore));

  requestAnimationFrame(restore);
}
