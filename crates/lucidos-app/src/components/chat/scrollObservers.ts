import {
  type NavScrollKind,
  lastWriteKind,
  isWhereWeHeldIt,
  isWhereWeLastScrolledIt,
  writeCount,
  forgetLeftPosition,
} from './scrollLedger';
import {
  isNavigationScroll,
  isPlacementScroll,
  markRevealScroll,
  scrollAnimating,
  landingGlideInFlight,
} from './scrollNavigation';
import { isAtLiveEdge, notAtTop, isScrollable, scrolledFromTop, awayFromBottom } from './scrollSurface';
import { readerGestureActive, attachReaderInput, readerScrollOwed } from './readerInput';
import {
  followState,
  wakeParkedRide,
  keepTheLiveEdge,
  transcriptIsLive,
  leaveTheRide,
  resumedRide,
} from './scrollRide';
import { scrollTakesRide, scrollPosition } from './scrollOwner';
import { isScrollKeyPress } from '../../utils/scrollActivity';
import { holdReadingLine, recordReadingLineOnScroll } from './scrollHold';
import {
  hasPendingLanding,
  honourLanding,
  landOnNextCard,
  glideToSubmittedTurn,
  landingInFlight,
  cancelLanding,
} from './scrollLanding';
import { isElementVisible } from '../../utils/elementVisibility';

export interface ScrollClass {
  navigation: boolean;
  kind: NavScrollKind | null;
  page: { navigation: boolean; kind: NavScrollKind | null };
  onLastWrite: boolean;
  onHeld: boolean;
  atEdge: boolean;
  gesture: boolean;
  placement: boolean;
  followSurvives: boolean;
}

export function classifyScroll(el?: HTMLElement | null): ScrollClass {
  const pageNavigation = isNavigationScroll();
  const page = { navigation: pageNavigation, kind: pageNavigation ? lastWriteKind() : null };
  if (!el) {
    return {
      ...page, page, onLastWrite: false, onHeld: false, atEdge: false,
      gesture: false, placement: false, followSurvives: false,
    };
  }
  const navigation = isNavigationScroll(el);
  const kind = navigation ? lastWriteKind(el) : null;
  const atEdge = isAtLiveEdge(el);
  const onHeld = isWhereWeHeldIt(el);
  const gesture = readerGestureActive(el);
  const placement = isPlacementScroll(el, navigation);
  const followSurvives = followState() === 'riding'
    && (onHeld || !scrollLeavesTheRide(atEdge, true, gesture, placement));
  return {
    navigation, kind, page, onLastWrite: isWhereWeLastScrolledIt(el), onHeld, atEdge,
    gesture, placement, followSurvives,
  };
}

function scrollLeavesTheRide(atEdge: boolean, tookOver: boolean, gesture: boolean, placement: boolean): boolean {
  return scrollTakesRide({ atEdge, tookOver, gesture, placement });
}

function liesInFullView(node: Node | null, container: HTMLElement): boolean {
  const box = node as HTMLElement | null;
  if (typeof box?.getBoundingClientRect !== 'function') return false;
  const r = box.getBoundingClientRect();
  const c = container.getBoundingClientRect();
  return r.height > 0 && r.top >= c.top && r.bottom <= c.bottom && r.left >= c.left && r.right <= c.right;
}

function attachReaderGestures(el: HTMLElement): () => void {
  if (typeof el.addEventListener !== 'function') return () => {};
  const detachInput = attachReaderInput(el);
  const onDescendantKey = (e: KeyboardEvent) => {
    if (e.target === el || !isScrollKeyPress(e)) return;
    markRevealScroll(el);
  };
  const onFocusIn = (e: FocusEvent) => {
    const to = e.target as Node | null;
    if (to === el || liesInFullView(to, el)) return;
    markRevealScroll(el);
  };

  el.addEventListener('keydown', onDescendantKey);
  el.addEventListener('focusin', onFocusIn, { passive: true });
  return () => {
    el.removeEventListener('keydown', onDescendantKey);
    el.removeEventListener('focusin', onFocusIn);
    detachInput();
  };
}

function honourGrowth(el: HTMLElement): void {
  if (hasPendingLanding()) {
    if (scrollAnimating() && !landingGlideInFlight()) return;
    honourLanding(el);
    return;
  }
  if ((!scrollAnimating() || landingGlideInFlight()) && landOnNextCard(el)) return;
  if (wakeParkedRide(el)) return;
  if (glideToSubmittedTurn(el)) return;
  keepTheLiveEdge(el);
}

export function makeScrollObservers(el: HTMLElement) {
  const detachGestures = attachReaderGestures(el);
  function isAtTop() {
    return el.scrollTop <= 80;
  }
  function isVisuallyAtTop() {
    return el.scrollTop <= 2;
  }
  function syncNotAtTop() {
    notAtTop.value = isScrollable(el) && !isAtTop();
  }
  function syncScrolledFromTop() {
    scrolledFromTop.value = isScrollable(el) && !isVisuallyAtTop();
  }

  let lastWidth = el.clientWidth;
  let lastHeight = el.clientHeight;

  function onScroll(scroll: ScrollClass = classifyScroll(el)): ScrollClass {
    if (!isElementVisible(el)) return scroll;
    const writesBefore = writeCount(el);
    const followBefore = followState();
    const tweenBefore = scrollAnimating();
    const { atEdge, gesture, placement } = scroll;
    const tookOver = !scroll.onHeld;
    const anchored = scroll.kind === 'anchor';
    forgetLeftPosition(el);
    const readerMoved = tookOver && (gesture || (placement && !anchored));
    scrollPosition.transition({ type: 'scroll', atEdge, tookOver, gesture, placement, live: transcriptIsLive() });
    if (followBefore === 'riding') {
      if (followState() !== 'riding') leaveTheRide();
      else if (!atEdge && tookOver) keepTheLiveEdge(el);
    } else if (followBefore === 'parked' && followState() === 'riding') {
      resumedRide(el);
    } else if (readerMoved && landingInFlight()) {
      cancelLanding();
    }
    syncNotAtTop();
    syncScrolledFromTop();
    awayFromBottom.value = !isAtLiveEdge(el);
    if (!scroll.onLastWrite || !anchored) recordReadingLineOnScroll(el);
    const acted = writeCount(el) !== writesBefore
      || followState() !== followBefore
      || (scrollAnimating()) !== tweenBefore;
    return acted ? classifyScroll(el) : scroll;
  }
  function onResize() {
    if (!isElementVisible(el)) return;
    const width = el.clientWidth;
    const height = el.clientHeight;
    const reflowed = width !== lastWidth;
    const boxChanged = reflowed || height !== lastHeight;
    lastWidth = width;
    lastHeight = height;
    const scrollOwed = followState() === 'riding' && readerScrollOwed(el) > 0;
    if (!scrollOwed && boxChanged) keepTheLiveEdge(el);
    holdReadingLine(el, reflowed);
    honourGrowth(el);
    syncNotAtTop();
    syncScrolledFromTop();
    awayFromBottom.value = !isAtLiveEdge(el);
  }
  return { onScroll, onResize, detachGestures };
}
