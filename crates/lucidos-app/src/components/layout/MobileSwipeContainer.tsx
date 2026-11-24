import { useRef, useEffect, useLayoutEffect } from 'preact/hooks';
import { mobileView, MOBILE_VIEWS, PANE_INDEX, PANE_COUNT, appPseudoFullscreen, type MobileView } from '../../store/store';
import { swipeToNeighbourPane } from '../../store/actions/pane';
import { MOBILE_PANE_CONFIGS } from './MobileAppHeader';
import { DashboardPanel } from '../dashboard/DashboardPanel';
import { EdgeSwipeZones } from './EdgeSwipeZones';
import { documentReplayHost, replayEdgeTap } from './edgeTapReplay';
import {
  isInteractiveTarget,
  opensSoftwareKeyboard,
  getRemPx,
  viewportIsKeyboardShrunk,
  readableVisualViewport,
  computedTranslateX,
} from '../../utils/dom';
import { PaneDrag, attachPaneDragTouches, isFrameInside, type PaneTrack, type TouchPoint } from './paneDrag';
import { subscribeFrameSwipes } from '../../store/actions/frame-swipe-bridge';
import { nativeFullscreenElement } from '../../store/appFullscreenHost';
import {
  noteKeyboardClosed,
  notePolledViewport,
  noteViewportResize,
  noteWakeViewport,
} from './keyboardCloseRelayout';
import { isPerfEnabled, onPerfOptInChange } from '../../utils/perfQueue';
import { reportNavigation } from '../../utils/navigationMarks';
import { useFocusedFieldVisible } from '../../hooks/useFocusedFieldVisible';

export const RUBBER_BAND = 0.3;

export const EDGE_NAV_GUARD_LEFT_REM = 2.5;

export const EDGE_NAV_GUARD_RIGHT_REM = 1.25;

export function shouldSuppressEdgeNavigation(args: {
  clientX: number;
  viewportWidth: number;
  remPx: number;
  targetIsInteractive: boolean;
  textInputFocused: boolean;
}): boolean {
  const { clientX, viewportWidth, remPx, targetIsInteractive, textInputFocused } = args;
  if (targetIsInteractive || textInputFocused) return false;
  return clientX <= EDGE_NAV_GUARD_LEFT_REM * remPx
    || clientX >= viewportWidth - EDGE_NAV_GUARD_RIGHT_REM * remPx;
}

export function shouldStartPaneSwipe(args: {
  textInputFocused: boolean;
  targetScrollable: boolean;
  appFullscreen: boolean;
}): boolean {
  const { textInputFocused, targetScrollable, appFullscreen } = args;
  return !textInputFocused && !targetScrollable && !appFullscreen;
}

export function isOffscreenPane(pane: MobileView, current: MobileView): boolean {
  return pane !== current;
}

export function isKeyboardActiveTarget(el: EventTarget | null): boolean {
  return (el as { tagName?: string } | null)?.tagName === 'TEXTAREA';
}

interface AttrTarget {
  setAttribute(name: string, value: string): void;
  removeAttribute(name: string): void;
}

export function reconcileKeyboardActive(root: AttrTarget, active: EventTarget | null): void {
  if (isKeyboardActiveTarget(active)) root.setAttribute('data-keyboard-active', '');
  else root.removeAttribute('data-keyboard-active');
}

export const KEYBOARD_RISE_GRACE_MS = 1000;

export function isGhostFocus(args: {
  fieldFocused: boolean;
  keyboardShrunk: boolean;
  focusHeldMs: number;
  hardwareKeyPressed: boolean;
}): boolean {
  const { fieldFocused, keyboardShrunk, focusHeldMs, hardwareKeyPressed } = args;
  return fieldFocused && !keyboardShrunk && !hardwareKeyPressed
    && focusHeldMs >= KEYBOARD_RISE_GRACE_MS;
}

export function computeAppHeight(args: KeyboardReading): number {
  return args.innerHeight - keyboardBandPx(args);
}

interface KeyboardReading {
  vvHeight: number;
  innerHeight: number;
  activeElementOpensKeyboard: boolean;
  blindBandPx?: number;
}

export function keyboardBandPx(args: KeyboardReading): number {
  const { vvHeight, innerHeight, activeElementOpensKeyboard, blindBandPx = 0 } = args;
  if (!activeElementOpensKeyboard) return 0;
  if (viewportIsKeyboardShrunk(vvHeight, innerHeight)) return innerHeight - vvHeight;
  return blindBandPx;
}

export function heldBandPx(args: {
  paddingPx: number;
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
  bandPx: number;
  heldPx?: number;
  anchorContentPx?: number;
}): number {
  const { paddingPx, scrollTop, scrollHeight, clientHeight, bandPx, heldPx = Infinity, anchorContentPx = 0 } = args;
  if (scrollTop <= 0 || scrollHeight - paddingPx < anchorContentPx - 1) return 0;
  const room = scrollHeight - clientHeight - scrollTop;
  const needed = Math.min(heldPx, Math.round(paddingPx - room));
  return needed > bandPx ? needed : 0;
}

const ASSUMED_KEYBOARD_FRACTION = 0.45;

export const KEYBOARD_BAND_STORAGE_KEY = 'lucidos-keyboard-band';

type Orientation = 'portrait' | 'landscape';

function loadStoredBands(storage: Pick<Storage, 'getItem'>): Record<Orientation, number> {
  const bands = { portrait: 0, landscape: 0 };
  try {
    const parsed: unknown = JSON.parse(storage.getItem(KEYBOARD_BAND_STORAGE_KEY) ?? 'null');
    if (typeof parsed !== 'object' || parsed === null) return bands;
    for (const key of ['portrait', 'landscape'] as const) {
      const px = (parsed as Record<string, unknown>)[key];
      if (typeof px === 'number' && Number.isFinite(px) && px > 0) bands[key] = px;
    }
  } catch (err) {
    console.warn('Keyboard band memory: ignoring unreadable stored bands', err);
  }
  return bands;
}

export function keyboardBandMemory(storage?: Pick<Storage, 'getItem' | 'setItem'>) {
  const widest = storage ? loadStoredBands(storage) : { portrait: 0, landscape: 0 };
  const stored = { ...widest };
  const orientationOf = (v: { width: number; height: number }): Orientation =>
    (v.width > v.height ? 'landscape' : 'portrait');
  return {
    note(bandPx: number, viewport: { width: number; height: number }) {
      const key = orientationOf(viewport);
      if (bandPx > widest[key]) widest[key] = bandPx;
      if (!storage || bandPx <= 0 || bandPx === stored[key]) return;
      stored[key] = bandPx;
      try {
        storage.setItem(KEYBOARD_BAND_STORAGE_KEY, JSON.stringify(stored));
      } catch (err) {
        console.warn('Keyboard band memory: could not store the measured band', err);
      }
    },
    reserve(viewport: { width: number; height: number }): number {
      return widest[orientationOf(viewport)] || Math.round(viewport.height * ASSUMED_KEYBOARD_FRACTION);
    },
  };
}

const KEYBOARD_POLL_MS = 3000;

function isHorizontallyScrollable(el: Element | null): boolean {
  while (el) {
    if (el.classList.contains('mobile-swipe-pane')) break;
    if (el.scrollWidth > el.clientWidth) {
      const style = getComputedStyle(el);
      if (style.overflowX === 'auto' || style.overflowX === 'scroll') return true;
    }
    el = el.parentElement;
  }
  return false;
}

function paneTransform(index: number, paneWidth: number): string {
  return paneWidth > 0 ? `translateX(${-index * paneWidth}px)` : `translateX(${-index * 100 / PANE_COUNT}%)`;
}

export function rubberBanded(offset: number, paneWidth: number): number {
  const minOffset = -(PANE_COUNT - 1) * paneWidth;
  if (offset > 0) return offset * RUBBER_BAND;
  if (offset < minOffset) return minOffset + (offset - minOffset) * RUBBER_BAND;
  return offset;
}

export function stripTrack(
  containerRef: { current: HTMLElement | null },
  trackRef: { current: HTMLElement | null },
  isHeld: () => boolean,
): PaneTrack {
  let base = 0;
  let heldWidth = 0;
  const paneWidth = () => containerRef.current?.offsetWidth ?? 0;
  return {
    paneWidth,
    grab() {
      const track = trackRef.current;
      if (!track) return;
      base = computedTranslateX(track);
      heldWidth = paneWidth();
      track.style.transition = 'none';
    },
    follow(dx) {
      const track = trackRef.current;
      if (!track) return;
      track.style.transition = 'none';
      track.style.transform = `translateX(${rubberBanded(base + dx, heldWidth)}px)`;
    },
    navigate: swipeToNeighbourPane,
    rest(animate) {
      const container = containerRef.current;
      const track = trackRef.current;
      if (!container || !track) return;
      container.scrollLeft = 0;
      track.style.transform = paneTransform(PANE_INDEX[mobileView.value], container.offsetWidth);
      if (animate) {
        track.style.transition = '';
        return;
      }
      track.style.transition = 'none';
      void track.offsetWidth;
      requestAnimationFrame(() => {
        if (!isHeld() && trackRef.current) trackRef.current.style.transition = '';
      });
    },
  };
}

export function targetOwnsHorizontalDrag(target: Element): boolean {
  return isHorizontallyScrollable(target) || !!target.closest('input[type="range"], .dashboard-tile-shield');
}

function gateStripTouch(e: TouchEvent, touch: TouchPoint): boolean {
  const target = e.target as Element;
  const textInputFocused = opensSoftwareKeyboard(document.activeElement);
  const targetScrollable = targetOwnsHorizontalDrag(target);

  if (!targetScrollable && shouldSuppressEdgeNavigation({
    clientX: touch.clientX,
    viewportWidth: window.innerWidth,
    remPx: getRemPx(),
    targetIsInteractive: isInteractiveTarget(target),
    textInputFocused,
  })) {
    e.preventDefault();
    if (e.defaultPrevented) replayEdgeTap(documentReplayHost(), e);
  }

  return shouldStartPaneSwipe({
    textInputFocused,
    targetScrollable,
    appFullscreen: appPseudoFullscreen.value,
  });
}

function appIsFullscreen(): boolean {
  return appPseudoFullscreen.value || nativeFullscreenElement() !== null;
}

export function trackAppHeight(): (() => void) | undefined {
  const vv = window.visualViewport;
  if (!vv) return;
  let lastSetHeight = -1;

  const setHeight = (h: number) => {
    if (h === lastSetHeight) return;
    lastSetHeight = h;
    document.documentElement.style.setProperty('--app-height', `${h}px`);
  };

  const bandMemory = keyboardBandMemory(window.localStorage);
  const layoutViewport = () => ({ width: window.innerWidth, height: window.innerHeight });

  // The visual viewport is *blind* from a zero or negative reading until it
  // next measures a keyboard. iOS can launch the PWA that way, even after a
  // few real readings, and keep the full height with the keys up until a
  // relaunch. Focus then decides alone, and the band is the remembered
  // keyboard. Once this page has measured a keyboard, a zero is a wake's
  // transient one and leaves this alone. Registered in
  // `docs/temporary-measures.md` § 1, "Blind visual viewport".
  let viewportBlind = false;
  let keyboardMeasured = false;
  const keyboardReading = (): KeyboardReading => {
    const activeElementOpensKeyboard = opensSoftwareKeyboard(document.activeElement);
    if (!readableVisualViewport()) {
      if (!keyboardMeasured) viewportBlind = true;
    } else if (viewportIsKeyboardShrunk(vv.height, window.innerHeight)) {
      keyboardMeasured = true;
      viewportBlind = false;
      if (activeElementOpensKeyboard) bandMemory.note(window.innerHeight - vv.height, layoutViewport());
    }
    return {
      vvHeight: vv.height,
      innerHeight: window.innerHeight,
      activeElementOpensKeyboard,
      blindBandPx: viewportBlind ? bandMemory.reserve(layoutViewport()) : 0,
    };
  };
  const layoutViewportKey = () => `${window.innerWidth}x${window.innerHeight}`;
  let measuredLayoutViewport = '';
  const currentAppHeight = () => {
    measuredLayoutViewport = layoutViewportKey();
    return computeAppHeight(keyboardReading());
  };
  let lastSetBand = -1;
  interface Hold { px: number; anchorContentPx: number; recheck: () => void; observer: MutationObserver }
  const holds = new Map<HTMLElement, Hold>();
  const releaseHold = (el: HTMLElement) => {
    const hold = holds.get(el);
    if (!hold) return;
    el.removeEventListener('scroll', hold.recheck);
    hold.observer.disconnect();
    el.style.removeProperty('--keyboard-band-hold');
    holds.delete(el);
  };
  const updateHold = (el: HTMLElement, bandPx: number) => {
    const paddingPx = parseFloat(getComputedStyle(el).paddingBottom) || 0;
    const hold = holds.get(el);
    const px = heldBandPx({
      paddingPx,
      scrollTop: el.scrollTop,
      scrollHeight: el.scrollHeight,
      clientHeight: el.clientHeight,
      bandPx,
      heldPx: hold?.px,
      anchorContentPx: hold?.anchorContentPx,
    });
    if (px === 0 || !el.isConnected) { releaseHold(el); return; }
    if (hold?.px === px) return;
    if (hold) hold.px = px;
    else {
      const recheck = () => updateHold(el, lastSetBand);
      const observer = new MutationObserver(recheck);
      observer.observe(el, { childList: true, subtree: true, characterData: true, attributes: true });
      holds.set(el, { px, anchorContentPx: el.scrollHeight - paddingPx, recheck, observer });
      el.addEventListener('scroll', recheck, { passive: true });
    }
    el.style.setProperty('--keyboard-band-hold', `${px}px`);
  };
  const holdScrollAnchors = (px: number) => {
    const scrollers = new Set(holds.keys());
    if (px < lastSetBand) {
      document.querySelectorAll<HTMLElement>('.mobile-swipe-pane .content-pane-body')
        .forEach((el) => scrollers.add(el));
    }
    for (const el of scrollers) updateHold(el, px);
  };
  const setBand = (px: number) => {
    if (px === lastSetBand) return;
    holdScrollAnchors(px);
    lastSetBand = px;
    document.documentElement.style.setProperty('--keyboard-band', `${px}px`);
  };
  const syncBand = () => setBand(keyboardBandPx(keyboardReading()));
  const syncShell = () => {
    setHeight(currentAppHeight());
    syncBand();
  };
  const armBand = (e: FocusEvent) => {
    if (!opensSoftwareKeyboard(e.target)) return;
    if (viewportBlind) syncShell();
    else setBand(bandMemory.reserve(layoutViewport()));
  };

  let observedHeight = vv.height;

  const onResize = () => {
    observedHeight = vv.height;
    setHeight(currentAppHeight());
    syncBand();
    noteViewportResize({ height: vv.height, layoutViewport: window.innerHeight });
  };
  const onLayoutViewportChange = () => {
    observedHeight = vv.height;
    setHeight(currentAppHeight());
    syncBand();
  };
  const onWindowResize = () => {
    if (layoutViewportKey() !== measuredLayoutViewport) onLayoutViewportChange();
  };
  const onFocusOut = (e: FocusEvent) => {
    if (opensSoftwareKeyboard(e.relatedTarget)) return;
    if (viewportBlind) syncShell();
    else syncBand();
  };
  const onWake = () => {
    const vvLooksShrunk =
      !readableVisualViewport() || viewportIsKeyboardShrunk(vv.height, window.innerHeight);
    const active = document.activeElement;
    if ((vvLooksShrunk || viewportBlind) && active instanceof HTMLElement && opensSoftwareKeyboard(active)) {
      active.blur();
    }
    lastSetHeight = -1;
    observedHeight = vv.height;
    setHeight(currentAppHeight());
    syncBand();
    if (vvLooksShrunk) noteKeyboardClosed();
    else noteWakeViewport({ height: vv.height, layoutViewport: window.innerHeight });
  };
  const onVisibilityChange = () => {
    if (document.visibilityState === 'visible') onWake();
  };
  let pollTimer: ReturnType<typeof setInterval> | null = null;
  const stopPoll = () => {
    if (pollTimer === null) return;
    clearInterval(pollTimer);
    pollTimer = null;
  };
  const pollViewport = () => {
    if (document.visibilityState === 'hidden') return;
    if (!isPerfEnabled()) { stopPoll(); return; }
    if (vv.height === observedHeight) return;
    observedHeight = vv.height;
    setHeight(currentAppHeight());
    syncBand();
    notePolledViewport({ height: vv.height, layoutViewport: window.innerHeight });
  };
  const startPoll = () => {
    if (pollTimer === null) pollTimer = setInterval(pollViewport, KEYBOARD_POLL_MS);
  };
  const unsubscribeOptIn = onPerfOptInChange((on) => (on ? startPoll() : stopPoll()));
  if (isPerfEnabled()) startPoll();
  vv.addEventListener('resize', onResize);
  window.addEventListener('orientationchange', onLayoutViewportChange);
  window.addEventListener('resize', onWindowResize);
  document.addEventListener('visibilitychange', onVisibilityChange);
  window.addEventListener('pageshow', onWake);
  document.addEventListener('focusin', armBand, { passive: true });
  document.addEventListener('focusout', onFocusOut, { passive: true });
  setHeight(currentAppHeight());
  syncBand();
  return () => {
    stopPoll();
    unsubscribeOptIn();
    vv.removeEventListener('resize', onResize);
    window.removeEventListener('orientationchange', onLayoutViewportChange);
    window.removeEventListener('resize', onWindowResize);
    document.removeEventListener('visibilitychange', onVisibilityChange);
    window.removeEventListener('pageshow', onWake);
    document.removeEventListener('focusin', armBand);
    document.removeEventListener('focusout', onFocusOut);
    for (const el of Array.from(holds.keys())) releaseHold(el);
    document.documentElement.style.removeProperty('--app-height');
    document.documentElement.style.removeProperty('--keyboard-band');
  };
}

export function MobileSwipeContainer() {
  const containerRef = useRef<HTMLDivElement>(null);
  const trackRef = useRef<HTMLDivElement>(null);
  const mountedRef = useRef(false);
  const dragRef = useRef<PaneDrag | null>(null);
  if (dragRef.current === null) {
    dragRef.current = new PaneDrag(stripTrack(containerRef, trackRef, () => dragRef.current?.holding ?? false), {
      frameMayLead: (source) => shouldStartPaneSwipe({
        textInputFocused: opensSoftwareKeyboard(document.activeElement),
        targetScrollable: false,
        appFullscreen: appIsFullscreen(),
      }) && isFrameInside(containerRef.current, source),
      mayCommit: () => !appIsFullscreen(),
    });
  }
  const drag = dragRef.current;

  useFocusedFieldVisible();

  useLayoutEffect(() => {
    drag.interrupt(mountedRef.current);
    mountedRef.current = true;
    reportNavigation('pane');
  }, [mobileView.value]);

  useEffect(() => {
    const track = trackRef.current;
    const container = containerRef.current;
    if (!track || !container) return;
    const onTransitionEnd = (e: TransitionEvent) => {
      if (e.target !== track || e.propertyName !== 'transform' || drag.holding) return;
      const paneWidth = container.offsetWidth;
      if (paneWidth <= 0) return;
      const correctValue = paneTransform(PANE_INDEX[mobileView.value], paneWidth);
      if (track.style.transform !== correctValue) {
        track.style.transform = correctValue;
      }
    };
    track.addEventListener('transitionend', onTransitionEnd);
    return () => track.removeEventListener('transitionend', onTransitionEnd);
  }, []);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    let lastPaneWidth = 0;
    const observer = new ResizeObserver(() => {
      const paneWidth = container.offsetWidth;
      if (paneWidth === 0 || paneWidth === lastPaneWidth) return;
      lastPaneWidth = paneWidth;
      drag.interrupt(false);
    });
    observer.observe(container);
    return () => observer.disconnect();
  }, []);

  const wasPseudoFullscreen = useRef(appPseudoFullscreen.value);
  useLayoutEffect(() => {
    if (wasPseudoFullscreen.current !== appPseudoFullscreen.value) drag.interrupt(false);
    wasPseudoFullscreen.current = appPseudoFullscreen.value;
  }, [appPseudoFullscreen.value]);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    return attachPaneDragTouches(el, drag, { gate: gateStripTouch });
  }, []);

  useEffect(() => subscribeFrameSwipes((source, message) => {
    if (message.kind === 'drag') drag.frameDrag(source, message.dx);
    else if (message.kind === 'release') drag.frameRelease(source, message.paneDelta);
    else drag.frameGone(source);
  }), []);


  useEffect(() => {
    const root = document.documentElement;
    let fieldFocusedAt = 0;
    let hardwareKeyPressed = false;
    const keyboardShrunk = (vv: VisualViewport) =>
      viewportIsKeyboardShrunk(vv.height, window.innerHeight);
    const onFocusIn = (e: FocusEvent) => {
      if (opensSoftwareKeyboard(e.target)) {
        fieldFocusedAt = performance.now();
        hardwareKeyPressed = false;
      }
      if (isKeyboardActiveTarget(e.target)) root.setAttribute('data-keyboard-active', '');
    };
    const onKeyDown = () => {
      const vv = window.visualViewport;
      if (vv && !keyboardShrunk(vv)) hardwareKeyPressed = true;
    };
    const releaseGhostFocus = (e: TouchEvent) => {
      const active = document.activeElement;
      const vv = window.visualViewport;
      if (!vv || !(active instanceof HTMLElement) || active.contains(e.target as Node)) return;
      if (isGhostFocus({
        fieldFocused: opensSoftwareKeyboard(active),
        keyboardShrunk: keyboardShrunk(vv),
        focusHeldMs: performance.now() - fieldFocusedAt,
        hardwareKeyPressed,
      })) active.blur();
    };
    const onFocusOut = (e: FocusEvent) => {
      if (!isKeyboardActiveTarget(e.relatedTarget)) root.removeAttribute('data-keyboard-active');
    };
    const reconcile = () => reconcileKeyboardActive(root, document.activeElement);
    const onTouchStart = (e: TouchEvent) => {
      releaseGhostFocus(e);
      reconcile();
    };
    document.addEventListener('focusin', onFocusIn, { passive: true });
    document.addEventListener('focusout', onFocusOut, { passive: true });
    document.addEventListener('keydown', onKeyDown, { passive: true, capture: true });
    document.addEventListener('touchstart', onTouchStart, { passive: true, capture: true });
    document.addEventListener('visibilitychange', reconcile, { passive: true });
    window.addEventListener('pageshow', reconcile, { passive: true });
    return () => {
      document.removeEventListener('focusin', onFocusIn);
      document.removeEventListener('focusout', onFocusOut);
      document.removeEventListener('keydown', onKeyDown, { capture: true });
      document.removeEventListener('touchstart', onTouchStart, { capture: true });
      document.removeEventListener('visibilitychange', reconcile);
      window.removeEventListener('pageshow', reconcile);
      root.removeAttribute('data-keyboard-active');
    };
  }, []);


  useEffect(() => {
    const onWindowScroll = () => {
      if (window.scrollY !== 0) window.scrollTo(0, 0);
    };
    window.addEventListener('scroll', onWindowScroll, { passive: true });
    return () => window.removeEventListener('scroll', onWindowScroll);
  }, []);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const onContainerScroll = () => {
      if (container.scrollLeft !== 0) container.scrollLeft = 0;
    };
    container.addEventListener('scroll', onContainerScroll, { passive: true });
    return () => container.removeEventListener('scroll', onContainerScroll);
  }, []);

  useEffect(trackAppHeight, []);

  return (
    <div class="mobile-swipe-wrapper">
      <div ref={containerRef} class="mobile-swipe-container">
        <div ref={trackRef} class="mobile-swipe-track">
          {MOBILE_VIEWS.map((v) => {
            const { Pane } = MOBILE_PANE_CONFIGS[v];
            return (
              <div key={v} class="mobile-swipe-pane" data-offscreen={isOffscreenPane(v, mobileView.value) ? '' : undefined}>
                <Pane />
                {v === 'thread' && <DashboardPanel />}
                <EdgeSwipeZones />
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
